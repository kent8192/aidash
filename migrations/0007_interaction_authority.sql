ALTER TABLE conversations ADD COLUMN created_by text NOT NULL DEFAULT 'human';
ALTER TABLE human_requests ADD COLUMN answered_by text;
