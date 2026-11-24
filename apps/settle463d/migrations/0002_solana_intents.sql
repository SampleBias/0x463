ALTER TABLE intents ADD COLUMN last_valid_block_height TEXT;
ALTER TABLE intents ADD COLUMN context_slot TEXT;
ALTER TABLE intents ADD COLUMN finalized_slot TEXT;
ALTER TABLE intents ADD COLUMN delivery_message TEXT;
ALTER TABLE intents ADD COLUMN verdict_message TEXT;
CREATE UNIQUE INDEX solana_signature ON intents(local_signature) WHERE last_valid_block_height IS NOT NULL;
