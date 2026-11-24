CREATE TABLE solana_preparations (
    order_id TEXT NOT NULL REFERENCES orders,
    action TEXT NOT NULL,
    message_digest TEXT NOT NULL,
    context_slot TEXT NOT NULL,
    last_valid_block_height TEXT NOT NULL,
    PRIMARY KEY(order_id, action, message_digest, context_slot, last_valid_block_height)
) WITHOUT ROWID;
