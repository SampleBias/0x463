ALTER TABLE intents ADD COLUMN known_terminal TEXT CHECK (known_terminal IN ('Paid','Refunded'));
