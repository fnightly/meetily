-- Migration: Notion integration token used by the sync feature.
-- Kept in the settings row next to the other provider keys so every secret in
-- the app is stored the same way.
ALTER TABLE settings ADD COLUMN notionApiKey TEXT;
