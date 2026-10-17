-- Migration: API key for SiliconFlow (SenseVoice) cloud transcription
ALTER TABLE transcript_settings ADD COLUMN siliconflowApiKey TEXT;
