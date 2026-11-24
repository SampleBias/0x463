"""Reproduce the public 0x463 signing fixtures with Python and Ed25519."""

import argparse
import json
from copy import deepcopy
from hashlib import sha256
from pathlib import Path

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

ROOT = Path(__file__).resolve().parents[2]
GOLDEN = ROOT / "fixtures/golden/positive-signing-vector.json"
EXAMPLES = ROOT / "spec/examples"
ENV = "sim:fixture"
PROFILE = "sha256-json-object-v1"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def canonical(value):
    def ordered(v):
        if isinstance(v, dict):
            return {k: ordered(v[k]) for k in sorted(v, key=lambda k: k.encode("utf-16-be"))}
        if isinstance(v, list):
            return [ordered(item) for item in v]
        if isinstance(v, (str, bool)):
            return v
        raise ValueError("fixture uses an unsupported protocol JSON type")

    return json.dumps(ordered(value), ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def preimage(domain, obj):
    return f"0x463/{domain}/0.1".encode("ascii") + b"\x00" + canonical(obj)


def unsigned(message):
    return {k: v for k, v in message.items() if k != "signature"}


def message_id(message):
    return sha256(preimage("signed", unsigned(message))).hexdigest()


def pubkey(key):
    return key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()


def sign(obj, key):
    return {**obj, "signature": key.sign(preimage("signed", obj)).hex()}


def display(obj):
    return (json.dumps(obj, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def base58(data):
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    value = int.from_bytes(data, "big")
    digits = ""
    while value:
        value, digit = divmod(value, 58)
        digits = alphabet[digit] + digits
    return "1" * (len(data) - len(data.lstrip(b"\x00"))) + digits


def check_artifacts():
    cases = (
        ("one-byte-change", 52, "d0d1d08718dc53380445736580bb4bcebcc56204656063add46afa9504a3670f"),
        ("extra-field", 64, "a6c4530d161e5de02e365783fe7dbb96d93e6334ba78f0b52257e0b7fa75635a"),
        ("number", 50, "58cda464d29ad6b71984742464aae5f6cfcd0c3855758c0d93ec9ce8f2305e74"),
        ("duplicate-key", 66, "8b67e670e01404b64c72e1814c32d549a23fc84c42271a8d589ba94b3e81c6dc"),
        ("malformed", 1, "021fb596db81e6d02bf3d2586ee3981fe519f275c0ac9ca76bbcf2ebb4097d96"),
    )
    for name, length, digest in cases:
        data = (ROOT / "fixtures/artifacts" / f"{name}.json").read_bytes()
        require(len(data) == length and sha256(data).hexdigest() == digest, f"{name} raw artifact differs")


def check_golden(g):
    require(GOLDEN.read_bytes() == (ROOT / "plan/golden" / GOLDEN.name).read_bytes(),
            "published golden vector differs from planning input")
    keys = {}
    for role in ("buyer", "seller", "verifier", "coordinator"):
        seed = sha256(b"0x463/fixture-seed/0.1\x00" + role.encode("ascii")).digest()
        key = Ed25519PrivateKey.from_private_bytes(seed)
        require(g["keys"][role] == dict(seed_hex=seed.hex(), public_key_hex=pubkey(key),
                                        public_key_base58=base58(bytes.fromhex(pubkey(key)))),
                f"{role} fixture identity differs")
        keys[role] = key

    raw = (ROOT / "fixtures/artifacts/reference.json").read_bytes()
    require(raw == g["artifact"]["utf8"].encode() and str(len(raw)) == g["artifact"]["length"]
            and sha256(raw).hexdigest() == g["artifact"]["sha256"], "reference artifact differs")
    require(g["order"]["object"] == {"environment": ENV, "buyer": pubkey(keys["buyer"]),
                                     "nonce": "11" * 32}, "golden order inputs differ")
    require(g["policy"]["object"]["expected_digest"] == sha256(raw).hexdigest()
            and g["policy"]["object"]["expected_length"] == str(len(raw)), "policy artifact differs")
    offer = g["signing"]["unsigned_object"]
    for section, domain, obj, field in (
        ("order", "order", g["order"]["object"], "order_id"),
        ("policy", "policy", g["policy"]["object"], "policy_hash"),
        ("signing", "signed", offer, "message_id"),
    ):
        image = preimage(domain, obj)
        stored = g[section]
        digest = sha256(image).hexdigest()
        require(canonical(obj).decode() == stored["canonical_utf8"]
                and image.hex() == stored["preimage_hex"] and digest == stored[field],
                f"{section} canonical bytes, preimage or digest differs")
        print(f"{field}={digest}")

    signed = sign(offer, keys["seller"])
    require(signed == g["signing"]["signed_message"]
            and signed["signature"] == g["signing"]["signature_hex"], "golden signature differs")
    signature = bytes.fromhex(signed["signature"])
    image = preimage("signed", offer)
    keys["seller"].public_key().verify(signature, image)
    mutated = image[:-1] + bytes([image[-1] ^ 1])
    require(sha256(mutated).digest() != sha256(image).digest(), "one-byte change retained the message ID")
    try:
        keys["seller"].public_key().verify(signature, mutated)
    except InvalidSignature:
        pass
    else:
        raise ValueError("signature accepted a one-byte change")

    require(canonical({"\ue000": "pua", "😀": "grin"}).hex() == g["unicode_key_order"]["canonical_utf8_hex"],
            "UTF-16 property order differs")
    separators = json.loads((ROOT / "plan/golden/jcs-line-separators.json").read_bytes())
    require(canonical(json.loads(separators["input_json"])).hex() == separators["canonical_utf8_hex"],
            "line separator encoding differs")
    return keys


def worked_transaction(g, keys):
    buyer, seller, verifier, issuer = (pubkey(keys[r]) for r in ("buyer", "seller", "verifier", "coordinator"))
    order = g["order"]["order_id"]
    policy = g["policy"]["object"]
    policy_hash = g["policy"]["policy_hash"]
    asset = {"decimals": "6", "id": "test-six-decimal", "kind": "sim"}
    amount = "2000000"
    messages = {"service_offer": g["signing"]["signed_message"]}

    def add(kind, role, body):
        key = keys[role]
        obj = dict(protocol="0x463", version="0.1", kind=kind, environment=ENV, issuer=pubkey(key), body=body)
        messages[kind] = sign(obj, key)
        return message_id(messages[kind])

    service = messages["service_offer"]["body"]["service_id"]
    offer_id = message_id(messages["service_offer"])
    request_id = add("request", "buyer", dict(
        order_id=order, nonce=g["order"]["object"]["nonce"], service_id=service, offer_id=offer_id,
        buyer=buyer, seller=seller, artifact_constraints=dict(verification_profile=PROFILE, max_bytes="1048576"),
        allowed_verifiers=[verifier], asset=asset, max_principal="2500000", expires_at="1700003600",
    ))
    deadlines = dict(fund_by="1700003600", deliver_by="1700007200", resolve_by="1700010800")
    buyer_dest, seller_dest = "sim:" + buyer, "sim:" + seller
    digest, length = policy["expected_digest"], policy["expected_length"]
    quote_id = add("quote", "seller", dict(
        request_id=request_id, order_id=order, offer_id=offer_id, buyer=buyer, seller=seller,
        verifier=verifier, receipt_issuer=issuer, service_id=service,
        artifact=dict(digest=digest, length=length, verification_profile=PROFILE, max_bytes="1048576"),
        acceptance_policy=policy, policy_hash=policy_hash, amount=amount, asset=asset,
        settlement=dict(asset_id=asset["id"], buyer_destination=buyer_dest, profile="sim-ledger-v1",
                        seller_destination=seller_dest),
        deadlines=deadlines, expires_at=deadlines["fund_by"], fees=dict(protocol_atomic="0", network_fee_payer="buyer"),
        artifact_store=dict(store_id="fixture-store", read_access="order-parties", retention_seconds="86400"),
    ))
    accept_id = add("accept", "buyer", dict(request_id=request_id, quote_id=quote_id, order_id=order,
                                            amount=amount, asset=asset, environment=ENV))
    delivery_id = add("delivery", "seller", dict(order_id=order, quote_id=quote_id, policy_hash=policy_hash,
                                                 digest=digest, length=length, content_id="22" * 32))
    verdict_id = add("verdict", "verifier", dict(
        order_id=order, quote_id=quote_id, delivery_id=delivery_id, policy_hash=policy_hash, algorithm=PROFILE,
        algorithm_version="1", decision="PASS", reason_codes=["MATCH"], observed_digest=digest, observed_length=length,
    ))
    terminal = dict(destination=seller_dest, ledger_kind="sim", terminal_locator=f"sim:{order}:3",
                    finality="simulated-final", principal=amount)
    add("receipt", "coordinator", dict(
        order_id=order, outcome="paid", asset=asset, buyer=buyer, seller=seller, verifier=verifier,
        receipt_issuer=issuer, request_id=request_id, quote_id=quote_id, acceptance_id=accept_id,
        environment=ENV, delivery_id=delivery_id, verdict_id=verdict_id, **terminal,
    ))
    bundle = dict(bundle_version="0.1", environment=ENV, order_id=order, request=messages["request"],
                  quote=messages["quote"], acceptance=messages["accept"], delivery=messages["delivery"],
                  verdict=messages["verdict"], receipt=messages["receipt"], observations=[],
                  ledger=dict(asset_id=asset["id"], decimals="6", deployment_id="fixture", clock="1700001800", **terminal))
    return messages, bundle


def negative_examples(messages, keys):
    offer = messages["service_offer"]
    quote = messages["quote"]
    seller = keys["seller"]
    files = {}
    files["uppercase-hex"] = display({**offer, "signature": offer["signature"].upper()})
    # Identical duplicates retain a valid signature in a parser that collapses keys.
    files["duplicate-key"] = display(offer).replace(b'"version": "0.1",', b'"version": "0.1",\n  "version": "0.1",', 1)
    files["unknown-field"] = display(sign({**unsigned(offer), "extra": True}, seller))
    files["wrong-version"] = display(sign({**unsigned(offer), "version": "0.2"}, seller))

    other = deepcopy(unsigned(offer))
    other["environment"] = "sim:other"
    other["body"]["environments"] = ["sim:other"]
    files["wrong-environment"] = display(sign(other, seller))

    numeric = deepcopy(unsigned(offer))
    image = preimage("signed", numeric).replace(b'"1700007200"', b'1700007200', 1)
    numeric["body"]["expires_at"] = 1700007200
    files["number-token"] = display({**numeric, "signature": seller.sign(image).hex()})

    for name, changes in [
        ("seller-as-verifier", {"verifier": pubkey(seller)}),
        ("above-ceiling", {"amount": "2500001"}),
        ("equal-deadlines", {"deadlines": {**quote["body"]["deadlines"],
                                           "deliver_by": quote["body"]["deadlines"]["fund_by"]}}),
    ]:
        obj = {**unsigned(quote), "body": {**quote["body"], **changes}}
        files[name] = display(sign(obj, seller))

    changed = {**offer, "body": {**offer["body"], "service_id": "sample-json-reporu"}}
    files["wrong-signature"] = display(changed)
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="regenerate the derived example files")
    args = parser.parse_args()
    golden = json.loads(GOLDEN.read_bytes())
    check_artifacts()
    keys = check_golden(golden)
    messages, bundle = worked_transaction(golden, keys)
    worked = EXAMPLES / "worked-transaction"
    outputs = {worked / f"{name}.json": display(msg) for name, msg in messages.items()}
    outputs[worked / "bundle.json"] = display(bundle)
    lines = ["# Worked fixture transaction", "",
             "Seven fixture messages describe a simulated payout of 2000000 atomic units in `sim:fixture`. "
             "The raw artifact is [reference.json](../../../fixtures/artifacts/reference.json). "
             "The fixture assigns content ID `22` repeated 32 times. Funding, delivery and release are the "
             "three modeled ledger events. The terminal clock is 1700001800 and the bundle has an empty observations array.",
             "", "| Signed message | Message ID |", "| --- | --- |"]
    for kind, msg in messages.items():
        lines.append(f"| [{kind}]({kind}.json) | `{message_id(msg)}` |")
    lines += ["", "The [bundle](bundle.json) packages the signed evidence. Its receipt has the message ID shown above."]
    outputs[worked / "README.md"] = ("\n".join(lines) + "\n").encode()
    negatives = negative_examples(messages, keys)
    for name, data in negatives.items():
        outputs[EXAMPLES / "negative" / f"{name}.json"] = data

    for path, data in outputs.items():
        if args.write:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        else:
            require(path.read_bytes() == data, f"fixture differs: {path.relative_to(ROOT)}")
    print(f"worked_messages={len(messages)} negative_examples={len(negatives)} receipt_id={message_id(messages['receipt'])}")


if __name__ == "__main__":
    main()
