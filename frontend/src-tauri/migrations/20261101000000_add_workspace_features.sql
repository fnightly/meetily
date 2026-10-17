-- Workspace features: speaker names, tags/projects, action items, full-text search.

-- Display names for diarization labels (speaker_1 → "张总"). Kept per meeting:
-- the same label number means a different person in every recording.
CREATE TABLE IF NOT EXISTS meeting_speakers (
    meeting_id TEXT NOT NULL,
    speaker TEXT NOT NULL,
    display_name TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (meeting_id, speaker),
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

-- Free-form tags. A meeting may carry several.
CREATE TABLE IF NOT EXISTS meeting_tags (
    meeting_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (meeting_id, tag),
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_meeting_tags_tag ON meeting_tags(tag);

-- One project (company / deal / workstream) per meeting.
CREATE TABLE IF NOT EXISTS meeting_projects (
    meeting_id TEXT PRIMARY KEY NOT NULL,
    project TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_meeting_projects_project ON meeting_projects(project);

-- Follow-ups extracted from summaries or added by hand.
CREATE TABLE IF NOT EXISTS action_items (
    id TEXT PRIMARY KEY NOT NULL,
    meeting_id TEXT,
    text TEXT NOT NULL,
    owner TEXT,
    due_date TEXT,
    done INTEGER NOT NULL DEFAULT 0,
    -- 'summary' when extracted, 'manual' when typed by the user
    source TEXT NOT NULL DEFAULT 'manual',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_action_items_meeting ON action_items(meeting_id);
CREATE INDEX IF NOT EXISTS idx_action_items_done ON action_items(done);

-- Meetings whose summary has already been mined for action items, so items
-- the user deleted are not resurrected by the next automatic import.
CREATE TABLE IF NOT EXISTS action_item_imports (
    meeting_id TEXT PRIMARY KEY NOT NULL,
    imported_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

-- Trigram full-text index over transcript text. Trigram tokenisation works for
-- Chinese (no word spacing) and makes substring search index-backed; queries
-- shorter than three characters fall back to LIKE in the repository.
CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
    transcript,
    content='transcripts',
    content_rowid='rowid',
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS transcripts_fts_ai AFTER INSERT ON transcripts BEGIN
    INSERT INTO transcripts_fts(rowid, transcript) VALUES (new.rowid, new.transcript);
END;
CREATE TRIGGER IF NOT EXISTS transcripts_fts_ad AFTER DELETE ON transcripts BEGIN
    INSERT INTO transcripts_fts(transcripts_fts, rowid, transcript) VALUES ('delete', old.rowid, old.transcript);
END;
CREATE TRIGGER IF NOT EXISTS transcripts_fts_au AFTER UPDATE OF transcript ON transcripts BEGIN
    INSERT INTO transcripts_fts(transcripts_fts, rowid, transcript) VALUES ('delete', old.rowid, old.transcript);
    INSERT INTO transcripts_fts(rowid, transcript) VALUES (new.rowid, new.transcript);
END;

INSERT INTO transcripts_fts(transcripts_fts) VALUES ('rebuild');
