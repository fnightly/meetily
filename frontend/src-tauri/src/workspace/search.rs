//! Meeting search across titles, labels, transcripts, summaries and notes.
//!
//! Transcript text goes through the trigram FTS index when the query has at
//! least three characters (the trigram minimum); shorter queries — common for
//! two-character Chinese words — fall back to an escaped LIKE scan. Hits are
//! grouped per meeting and ranked, so a meeting that mentions the query fifty
//! times appears once with its best snippets rather than fifty times.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::SqlitePool;

use super::text::{count_ci, find_ci, fts_phrase, like_pattern, snippet_around, summary_markdown};
use crate::state::AppState;

const SNIPPET_RADIUS: usize = 36;
const MAX_TRANSCRIPT_ROWS: i64 = 3000;
const TRANSCRIPT_HITS_PER_MEETING: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchHit {
    /// "title" | "label" | "summary" | "notes" | "transcript"
    pub source: String,
    pub snippet: String,
    pub transcript_id: Option<String>,
    pub audio_start_time: Option<f64>,
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetingSearchResult {
    pub meeting_id: String,
    pub title: String,
    pub created_at: String,
    pub hits: Vec<SearchHit>,
    pub total_hits: usize,
    pub score: f64,
}

#[derive(Default)]
struct Bucket {
    hits: Vec<SearchHit>,
    transcript_hits: usize,
    total: usize,
    score: f64,
}

impl Bucket {
    fn add(&mut self, hit: SearchHit, weight: f64) {
        self.total += 1;
        self.score += weight;
        if hit.source == "transcript" {
            self.transcript_hits += 1;
            if self.transcript_hits > TRANSCRIPT_HITS_PER_MEETING {
                return;
            }
        }
        self.hits.push(hit);
    }
}

fn source_rank(source: &str) -> u8 {
    match source {
        "title" => 0,
        "label" => 1,
        "summary" => 2,
        "notes" => 3,
        _ => 4,
    }
}

async fn transcript_rows(
    pool: &SqlitePool,
    query: &str,
) -> Result<Vec<(String, String, String, Option<f64>, Option<String>)>, sqlx::Error> {
    if query.chars().count() >= 3 {
        let fts = sqlx::query_as::<_, (String, String, String, Option<f64>, Option<String>)>(
            "SELECT t.id, t.meeting_id, t.transcript, t.audio_start_time, t.speaker
             FROM transcripts_fts f JOIN transcripts t ON t.rowid = f.rowid
             WHERE transcripts_fts MATCH ?
             ORDER BY t.meeting_id, t.audio_start_time
             LIMIT ?",
        )
        .bind(fts_phrase(query))
        .bind(MAX_TRANSCRIPT_ROWS)
        .fetch_all(pool)
        .await;
        match fts {
            Ok(rows) => return Ok(rows),
            Err(e) => log::warn!("FTS search failed ({e}); falling back to LIKE"),
        }
    }
    sqlx::query_as::<_, (String, String, String, Option<f64>, Option<String>)>(
        "SELECT id, meeting_id, transcript, audio_start_time, speaker FROM transcripts
         WHERE transcript LIKE ? ESCAPE '\\'
         ORDER BY meeting_id, audio_start_time
         LIMIT ?",
    )
    .bind(like_pattern(query))
    .bind(MAX_TRANSCRIPT_ROWS)
    .fetch_all(pool)
    .await
}

pub async fn search(pool: &SqlitePool, raw_query: &str, limit: usize) -> Result<Vec<MeetingSearchResult>, sqlx::Error> {
    let query = raw_query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let like = like_pattern(query);
    let meetings: HashMap<String, (String, String)> =
        sqlx::query_as::<_, (String, String, String)>("SELECT id, title, created_at FROM meetings")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(id, title, created)| (id, (title, created)))
            .collect();
    let mut buckets: HashMap<String, Bucket> = HashMap::new();
    let simple = |source: &str, snippet: String| SearchHit {
        source: source.to_string(),
        snippet,
        transcript_id: None,
        audio_start_time: None,
        speaker: None,
    };

    for (id, (title, _)) in &meetings {
        if find_ci(title, query).is_some() {
            buckets.entry(id.clone()).or_default().add(simple("title", title.clone()), 10.0);
        }
    }

    for (meeting_id, label) in sqlx::query_as::<_, (String, String)>(
        "SELECT meeting_id, tag FROM meeting_tags WHERE tag LIKE ? ESCAPE '\\'
         UNION ALL SELECT meeting_id, project FROM meeting_projects WHERE project LIKE ? ESCAPE '\\'",
    )
    .bind(&like)
    .bind(&like)
    .fetch_all(pool)
    .await?
    {
        buckets.entry(meeting_id).or_default().add(simple("label", label), 6.0);
    }

    for (meeting_id, result) in sqlx::query_as::<_, (String, String)>(
        "SELECT meeting_id, result FROM summary_processes WHERE result IS NOT NULL AND result LIKE ? ESCAPE '\\'",
    )
    .bind(&like)
    .fetch_all(pool)
    .await?
    {
        if let Some(md) = summary_markdown(&result) {
            let n = count_ci(&md, query);
            if n > 0 {
                let snippet = snippet_around(&md, query, SNIPPET_RADIUS);
                buckets.entry(meeting_id).or_default().add(simple("summary", snippet), 4.0 + n.min(5) as f64);
            }
        }
    }

    for (meeting_id, notes) in sqlx::query_as::<_, (String, String)>(
        "SELECT meeting_id, notes_markdown FROM meeting_notes WHERE notes_markdown LIKE ? ESCAPE '\\'",
    )
    .bind(&like)
    .fetch_all(pool)
    .await?
    {
        if find_ci(&notes, query).is_some() {
            let snippet = snippet_around(&notes, query, SNIPPET_RADIUS);
            buckets.entry(meeting_id).or_default().add(simple("notes", snippet), 3.0);
        }
    }

    for (id, meeting_id, text, start, speaker) in transcript_rows(pool, query).await? {
        // LIKE is ASCII-case-insensitive only and FTS may match across case
        // folding differently, so confirm with the same matcher as snippets.
        if find_ci(&text, query).is_none() {
            continue;
        }
        let hit = SearchHit {
            source: "transcript".into(),
            snippet: snippet_around(&text, query, SNIPPET_RADIUS),
            transcript_id: Some(id),
            audio_start_time: start,
            speaker,
        };
        buckets.entry(meeting_id).or_default().add(hit, 1.0);
    }

    let mut results: Vec<MeetingSearchResult> = buckets
        .into_iter()
        .filter_map(|(meeting_id, mut bucket)| {
            let (title, created_at) = meetings.get(&meeting_id)?.clone();
            bucket.hits.sort_by_key(|h| source_rank(&h.source));
            Some(MeetingSearchResult {
                meeting_id,
                title,
                created_at,
                hits: bucket.hits,
                total_hits: bucket.total,
                // Diminishing returns for many transcript mentions.
                score: bucket.score.min(30.0),
            })
        })
        .collect();
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });
    results.truncate(limit.clamp(1, 200));
    Ok(results)
}

#[tauri::command]
pub async fn api_search_meetings(
    query: String,
    limit: Option<usize>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<MeetingSearchResult>, String> {
    search(state.db_manager.pool(), &query, limit.unwrap_or(40))
        .await
        .map_err(|e| format!("Search failed: {e}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    pub async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    pub async fn seed(pool: &SqlitePool) {
        for (id, title, created) in [
            ("m1", "Acme 融资沟通", "2026-01-05T10:00:00Z"),
            ("m2", "周会", "2026-01-06T10:00:00Z"),
        ] {
            sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
                .bind(id).bind(title).bind(created).bind(created)
                .execute(pool).await.unwrap();
        }
        let rows = [
            ("t1", "m1", "我们讨论了估值，Acme 希望估值在五亿左右", 12.0, Some("speaker_1")),
            ("t2", "m1", "条款方面需要再确认清算优先权", 40.0, Some("speaker_2")),
            ("t3", "m2", "本周进度正常，下周讨论估值模型", 5.0, None),
            ("t4", "m2", "Remember the Pricing deck", 9.0, None),
        ];
        for (id, mid, text, start, speaker) in rows {
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, speaker) VALUES (?, ?, ?, '2026-01-05T10:00:00Z', ?, ?)")
                .bind(id).bind(mid).bind(text).bind(start).bind(speaker)
                .execute(pool).await.unwrap();
        }
        sqlx::query("INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) VALUES ('m2', 'completed', 'x', 'x', ?)")
            .bind(r###"{"markdown":"## 决议\n- 下周完成估值模型"}"###)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn groups_hits_per_meeting_and_ranks_titles_first() {
        let pool = test_pool().await;
        seed(&pool).await;
        let results = search(&pool, "估值", 10).await.unwrap();
        assert_eq!(results.len(), 2);
        // m2 has a summary hit (4+1) + transcript (1) = 6; m1 has 2 transcript rows = 2.
        assert_eq!(results[0].meeting_id, "m2");
        assert_eq!(results[0].hits[0].source, "summary");
        let m1 = results.iter().find(|r| r.meeting_id == "m1").unwrap();
        assert_eq!(m1.total_hits, 1);
        assert_eq!(m1.hits[0].audio_start_time, Some(12.0));
        assert_eq!(m1.hits[0].speaker.as_deref(), Some("speaker_1"));

        let by_title = search(&pool, "acme", 10).await.unwrap();
        assert_eq!(by_title[0].meeting_id, "m1");
        assert_eq!(by_title[0].hits[0].source, "title");
    }

    #[tokio::test]
    async fn fts_index_follows_inserts_updates_and_deletes() {
        let pool = test_pool().await;
        seed(&pool).await;
        // Three+ characters → FTS path; case-insensitive.
        let hits = search(&pool, "pricing deck", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        sqlx::query("UPDATE transcripts SET transcript = 'nothing here' WHERE id = 't4'")
            .execute(&pool).await.unwrap();
        assert!(search(&pool, "pricing deck", 10).await.unwrap().is_empty());
        sqlx::query("DELETE FROM transcripts WHERE id = 't2'").execute(&pool).await.unwrap();
        assert!(search(&pool, "清算优先权", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn wildcards_in_queries_are_literal() {
        let pool = test_pool().await;
        seed(&pool).await;
        assert!(search(&pool, "%", 10).await.unwrap().is_empty());
        assert!(search(&pool, "  ", 10).await.unwrap().is_empty());
    }
}
