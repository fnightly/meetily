-- The legacy `recordings` table was never populated by the current app and is
-- only rewritten (never inserted) by backup restore. Path rewriting now uses
-- `meetings.folder_path`, so drop the dead table.
DROP TABLE IF EXISTS recordings;
