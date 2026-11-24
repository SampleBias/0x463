CREATE TABLE deployment (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1), environment TEXT NOT NULL,
  genesis_clock TEXT NOT NULL, clock TEXT NOT NULL, buyer TEXT NOT NULL,
  opening_balance TEXT NOT NULL, receipt_issuer TEXT NOT NULL
);
CREATE TABLE signed_messages (
  message_id TEXT PRIMARY KEY, kind TEXT NOT NULL, environment TEXT NOT NULL,
  issuer TEXT NOT NULL, signed_bytes BLOB NOT NULL, canonical BLOB NOT NULL,
  signature TEXT NOT NULL, order_id TEXT, service_id TEXT
);
CREATE INDEX messages_by_order ON signed_messages(order_id,kind);
CREATE TABLE acceptances (
  environment TEXT NOT NULL, buyer TEXT NOT NULL, nonce TEXT NOT NULL,
  request_id TEXT NOT NULL UNIQUE REFERENCES signed_messages, order_id TEXT NOT NULL UNIQUE,
  PRIMARY KEY(environment,buyer,nonce)
);
CREATE TABLE orders (
  order_id TEXT PRIMARY KEY REFERENCES acceptances(order_id), request_id TEXT NOT NULL REFERENCES signed_messages,
  quote_id TEXT NOT NULL REFERENCES signed_messages, acceptance_id TEXT NOT NULL REFERENCES signed_messages,
  buyer TEXT NOT NULL, seller TEXT NOT NULL, verifier TEXT NOT NULL, receipt_issuer TEXT NOT NULL,
  principal TEXT NOT NULL, buyer_destination TEXT NOT NULL, seller_destination TEXT NOT NULL,
  state TEXT NOT NULL, held TEXT NOT NULL DEFAULT '0', paid TEXT NOT NULL DEFAULT '0', refunded TEXT NOT NULL DEFAULT '0',
  fund_by TEXT NOT NULL, deliver_by TEXT NOT NULL, resolve_by TEXT NOT NULL,
  delivery_id TEXT, verdict_id TEXT, definitive_id TEXT, terminal_locator TEXT, terminal_clock TEXT,
  version INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE artifacts (
  content_id TEXT PRIMARY KEY, order_id TEXT NOT NULL REFERENCES orders, digest TEXT NOT NULL,
  length TEXT NOT NULL, storage_key TEXT NOT NULL, UNIQUE(order_id,digest)
);
CREATE TABLE challenges (
  challenge_id TEXT PRIMARY KEY, nonce TEXT NOT NULL, public_key TEXT NOT NULL, role TEXT NOT NULL,
  expires_at INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE audit_events (
  order_id TEXT NOT NULL, seq INTEGER NOT NULL, event_type TEXT NOT NULL, payload_hash TEXT NOT NULL,
  prev_digest TEXT NOT NULL, event_digest TEXT NOT NULL, observation_id TEXT NOT NULL REFERENCES signed_messages,
  PRIMARY KEY(order_id,seq)
);
CREATE TABLE intents (
  intent_id TEXT PRIMARY KEY, order_id TEXT NOT NULL REFERENCES orders, action TEXT NOT NULL,
  state TEXT NOT NULL, signed_bytes BLOB NOT NULL, local_signature TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
  execution_clock TEXT, ledger_seq INTEGER UNIQUE
);
CREATE UNIQUE INDEX active_intent ON intents(order_id,action) WHERE state IN ('prepared','submitted','unknown');
CREATE UNIQUE INDEX successful_action ON intents(order_id,action) WHERE state='finalized_success';
CREATE TABLE outbox (
  id INTEGER PRIMARY KEY, intent_id TEXT NOT NULL UNIQUE REFERENCES intents, available_at TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0, locked_until TEXT, last_error TEXT
);
CREATE TABLE receipts (order_id TEXT PRIMARY KEY REFERENCES orders, receipt_id TEXT NOT NULL UNIQUE REFERENCES signed_messages, bundle BLOB NOT NULL);
CREATE TABLE idempotency (
  caller TEXT NOT NULL, operation TEXT NOT NULL, idem_key TEXT NOT NULL, path TEXT NOT NULL,
  request_hash TEXT NOT NULL, status INTEGER NOT NULL, response BLOB NOT NULL,
  PRIMARY KEY(caller,operation,idem_key)
);
ALTER TABLE idempotency ADD COLUMN intent_id TEXT REFERENCES intents;
