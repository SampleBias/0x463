# Independent fixture reproduction

Run `python3 fixtures/tools/independent_vector.py` from the project root with Python 3 and the Python `cryptography` package installed. The checker uses `hashlib` for SHA-256 and `cryptography` for Ed25519. It runs offline, checks the length and SHA-256 of all six raw artifacts, and uses the public deterministic test seeds in the golden vector.

The default run checks the original vector copy, canonical bytes, keys, signatures, message IDs, Unicode cases and the derived worked and negative files. A mismatch exits nonzero. Its final line prints the worked receipt ID. The signed bundle represents a constructed simulator example.

Use `python3 fixtures/tools/independent_vector.py --write` to regenerate the derived example files after an intentional fixture change. The original planning vector and the raw artifact inputs are read-only inputs to both modes.
