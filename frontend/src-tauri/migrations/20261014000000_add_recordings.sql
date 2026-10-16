-- Recording history.
--
-- Each row is one captured audio recording. Recordings are independent of
-- meetings: a recording can exist before it is transcribed, transcription and
-- summarization are explicit user actions, and the underlying audio file lives
-- in the recordings storage folder rather than in the database.
--
-- `title` defaults to the date/time captured at creation
-- (see `recordings::default_title`), and can be renamed by the user.
CREATE TABLE IF NOT EXISTS recordings (
    id TEXT PRIMARY KEY NOT NULL,
    -- User-visible name. Defaults to a date/time stamp.
    title TEXT NOT NULL,
    -- Absolute path to the audio file on disk.
    file_path TEXT,
    -- Optional link to the meeting this recording was transcribed into.
    meeting_id TEXT,
    -- Duration in seconds, when known.
    duration REAL,
    -- Byte size of the audio file at capture time, when known.
    file_size INTEGER,
    -- Absolute path of the folder this recording's artifacts live in.
    folder_path TEXT,
    -- 'none' | 'pending' | 'processing' | 'done' | 'error'
    transcription_status TEXT NOT NULL DEFAULT 'none',
    -- 'none' | 'pending' | 'processing' | 'done' | 'error'
    summary_status TEXT NOT NULL DEFAULT 'none',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- The sidebar lists recordings newest-first; this keeps that ordered scan cheap.
CREATE INDEX IF NOT EXISTS idx_recordings_created_at ON recordings(created_at DESC);
-- Detail lookups and future joins resolve a recording from its meeting.
CREATE INDEX IF NOT EXISTS idx_recordings_meeting_id ON recordings(meeting_id);
