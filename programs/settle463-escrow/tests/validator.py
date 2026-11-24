"""Exercise the escrow ABI and classic SPL Token against a fresh private validator."""

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import hashlib
import itertools
import json
from pathlib import Path
import secrets
import struct
import time
from urllib.parse import urlsplit
from urllib.request import ProxyHandler, Request, build_opener

from solders.compute_budget import set_compute_unit_price
from solders.hash import Hash
from solders.instruction import AccountMeta, Instruction
from solders.keypair import Keypair
from solders.pubkey import Pubkey
from solders.system_program import create_account, transfer
from solders.transaction import Transaction

TOKEN = Pubkey.from_string("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
ATA = Pubkey.from_string("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
SYSTEM = Pubkey.default()
CLOCK = Pubkey.from_string("SysvarC1ock11111111111111111111111111111111")
PRINCIPAL = 2_000_000


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def meta(key, signer=False, write=False):
    return AccountMeta(key, signer, write)


def ata(owner, mint):
    return Pubkey.find_program_address([bytes(owner), bytes(TOKEN), bytes(mint)], ATA)[0]


def create_ata(payer, owner, mint):
    return Instruction(ATA, bytes([1]), [
        meta(payer, True, True), meta(ata(owner, mint), write=True), meta(owner),
        meta(mint), meta(SYSTEM), meta(TOKEN),
    ])


def token_ix(data, *accounts):
    return Instruction(TOKEN, data, list(accounts))


def segment(name):
    return hashlib.sha256(("global:" + name).encode()).digest()[:8]


def load_key(path):
    return Keypair.from_bytes(bytes(json.loads(path.read_text())))


@dataclass
class Order:
    buyer: Keypair
    seller: Keypair
    verifier: Keypair
    nonce: bytes
    escrow: Pubkey
    vault: Pubkey
    mint: Pubkey
    fund_by: int
    deliver_by: int
    resolve_by: int
    ids: bytes
    principal: int

    def fund(self, env, program, config, token=TOKEN, mint=None, principal=None):
        env = env.encode()
        key = self.buyer.pubkey()
        mint = self.mint if mint is None else mint
        principal = self.principal if principal is None else principal
        args = self.nonce + self.ids + bytes(self.verifier.pubkey())
        args += struct.pack("<QqqqI", principal, self.fund_by, self.deliver_by, self.resolve_by, len(env)) + env
        return Instruction(program, segment("fund") + args, [
            meta(key, True, True), meta(self.seller.pubkey(), True), meta(config),
            meta(self.escrow, write=True), meta(mint), meta(ata(key, self.mint), write=True),
            meta(ata(self.seller.pubkey(), self.mint)), meta(self.vault, write=True), meta(token), meta(SYSTEM), meta(ATA),
        ])

    def deliver(self, program, delivery_id):
        return Instruction(program, segment("commit_delivery") + delivery_id,
                           [meta(self.seller.pubkey(), True), meta(self.escrow, write=True)])

    def terminal(self, program, *, caller, release=False, delivery_id=None, verdict_id=None):
        action = "release" if release else "refund"
        owner = self.seller.pubkey() if release else self.buyer.pubkey()
        args = self.nonce + (delivery_id + verdict_id if release else b"")
        return Instruction(program, segment(action) + args, [
            meta(caller, True), meta(self.escrow, write=True), meta(self.mint),
            meta(self.vault, write=True), meta(ata(owner, self.mint), write=True), meta(TOKEN),
        ])


class Validator:
    def __init__(self, deployment, keydir):
        self.rpc_url = deployment["rpc_url"]
        url = urlsplit(self.rpc_url)
        require(url.scheme == "http" and url.hostname in ("127.0.0.1", "::1"), "expected loopback fixture")
        self.mint = Pubkey.from_string(deployment["mint"])
        self.program = Pubkey.from_string(deployment["program_id"])
        self.config = Pubkey.from_string(deployment["config"])
        self.genesis = self.rpc("getGenesisHash")
        require(self.genesis == deployment["genesis"] and deployment["token_program"] == str(TOKEN), "deployment pins")
        self.env = f"solana:{self.genesis}:{self.program}:{self.config}"
        self.admin = load_key(keydir / "fee-payer.json")
        self.authority = load_key(keydir / "mint-authority.json")
        self.priority = itertools.count(1)
        self.signatures = []
        self.evidence = {}

    def rpc(self, method, *params):
        request = Request(self.rpc_url, data=json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params)).encode(),
                          headers={"content-type": "application/json"})
        with build_opener(ProxyHandler({})).open(request, timeout=15) as stream:
            value = json.load(stream)
        require("error" not in value, f"{method}: {value.get('error')}")
        return value["result"]

    def account(self, key, commitment="confirmed"):
        return self.rpc("getAccountInfo", str(key), {"encoding": "base64", "commitment": commitment})["value"]

    def data(self, key, commitment="confirmed"):
        account = self.account(key, commitment)
        require(account is not None, f"missing account {key}")
        return base64.b64decode(account["data"][0], validate=True)

    def amount(self, key):
        return struct.unpack_from("<Q", self.data(key), 64)[0]

    def clock_sample(self, min_slot=0):
        result = self.rpc("getAccountInfo", str(CLOCK), {"encoding": "base64", "commitment": "processed",
                                                         "minContextSlot": min_slot})
        data = base64.b64decode(result["value"]["data"][0], validate=True)
        return result["context"]["slot"], struct.unpack_from("<q", data, 32)[0]

    def now(self):
        return self.clock_sample()[1]

    def wait_clock(self, target):
        end = time.monotonic() + 90
        while time.monotonic() < end:
            if self.now() >= target:
                return
            time.sleep(0.035)
        raise TimeoutError(f"validator clock did not reach {target}")

    def signed(self, ixs, payer, *others):
        blockhash = self.rpc("getLatestBlockhash", {"commitment": "confirmed"})["value"]["blockhash"]
        signers = {key.pubkey(): key for key in (payer, *others)}
        # Separate instructions in the same slot must also have distinct transaction identities.
        return Transaction.new_signed_with_payer(
            [set_compute_unit_price(next(self.priority)), *ixs], payer.pubkey(), list(signers.values()), Hash.from_string(blockhash),
        )

    def submit(self, tx, errors=()):
        wire = base64.b64encode(bytes(tx)).decode()
        signature = self.rpc("sendTransaction", wire, {"encoding": "base64", "skipPreflight": True, "maxRetries": 5})
        require(signature == str(tx.signatures[0]), "RPC signature differs from signed transaction")
        end = time.monotonic() + 45
        while time.monotonic() < end:
            item = self.rpc("getSignatureStatuses", [signature])["value"][0]
            if item is not None and item["confirmationStatus"] in ("confirmed", "finalized"):
                error = item["err"]
                if errors:
                    instruction = (error or {}).get("InstructionError")
                    require(isinstance(instruction, list) and instruction[1] in [{"Custom": n} for n in errors],
                            f"expected {errors}, got {error}, signature={signature}")
                else:
                    require(error is None, f"unexpected transaction error {error}, signature={signature}")
                self.signatures.append(signature)
                return signature
            time.sleep(0.15)
        raise TimeoutError(f"unconfirmed transaction {signature}")

    def send(self, ixs, payer=None, *others, errors=()):
        return self.submit(self.signed(ixs, self.admin if payer is None else payer, *others), errors)

    def finalize(self, signatures):
        end = time.monotonic() + 65
        while time.monotonic() < end:
            vals = self.rpc("getSignatureStatuses", signatures)["value"]
            if all(v and v["confirmationStatus"] == "finalized" for v in vals):
                return
            time.sleep(0.4)
        raise TimeoutError("validator did not finalize the tested transactions")

    def transaction(self, signature):
        end = time.monotonic() + 15
        while time.monotonic() < end:
            tx = self.rpc("getTransaction", signature, {"encoding": "json", "commitment": "confirmed",
                                                       "maxSupportedTransactionVersion": 0})
            if tx is not None:
                return tx
            time.sleep(0.1)
        raise TimeoutError(f"missing confirmed transaction {signature}")

    def mint_to(self, owner, amount):
        ix = token_ix(bytes([7]) + struct.pack("<Q", amount), meta(self.mint, write=True),
                      meta(ata(owner, self.mint), write=True), meta(self.authority.pubkey(), True))
        self.send([create_ata(self.admin.pubkey(), owner, self.mint), ix], self.admin, self.authority)

    def parties(self, amount):
        buyer, seller, verifier = Keypair(), Keypair(), Keypair()
        self.send([transfer(dict(from_pubkey=self.admin.pubkey(), to_pubkey=buyer.pubkey(), lamports=100_000_000)),
                   create_ata(self.admin.pubkey(), seller.pubkey(), self.mint)])
        self.mint_to(buyer.pubkey(), amount)
        return buyer, seller, verifier

    def order(self, parties, deadlines, principal=PRINCIPAL):
        buyer, seller, verifier = parties
        nonce = secrets.token_bytes(32)
        escrow = Pubkey.find_program_address([b"settle463", b"escrow", b"v1", bytes(buyer.pubkey()), nonce], self.program)[0]
        return Order(buyer, seller, verifier, nonce, escrow, ata(escrow, self.mint), self.mint,
                     *deadlines, secrets.token_bytes(5 * 32), principal)

    def funding(self, order, **changes):
        return [create_ata(order.buyer.pubkey(), order.escrow, self.mint),
                order.fund(self.env, self.program, self.config, **changes)]

    def fund(self, order, **changes):
        return self.send(self.funding(order, **changes), order.buyer, order.seller)

    def close(self, key):
        owner = key.pubkey()
        self.send([token_ix(bytes([9]), meta(ata(owner, self.mint), write=True), meta(self.admin.pubkey(), write=True),
                            meta(owner, True))], self.admin, key)

    def dust(self, dust_key, order):
        ix = token_ix(bytes([12]) + struct.pack("<QB", 1, 6), meta(ata(dust_key.pubkey(), self.mint), write=True),
                      meta(self.mint), meta(order.vault, write=True), meta(dust_key.pubkey(), True))
        self.send([ix], self.admin, dust_key)

    def wrong_mint(self):
        mint = Keypair()
        rent = self.rpc("getMinimumBalanceForRentExemption", 82)
        create = create_account(dict(from_pubkey=self.admin.pubkey(), to_pubkey=mint.pubkey(), lamports=rent,
                                     space=82, owner=TOKEN))
        init = token_ix(bytes([20, 9]) + bytes(self.authority.pubkey()) + bytes([0]), meta(mint.pubkey(), write=True))
        self.send([create, init], self.admin, mint)
        return mint.pubkey()


def program_cases(v):
    parts = v.parties(PRINCIPAL)
    buyer, seller, verifier = parts
    mint = v.mint
    wrong_mint = v.wrong_mint()
    dust_key = Keypair()
    v.mint_to(dust_key.pubkey(), 2)
    now = v.now()
    order = v.order(parts, (now + 100, now + 120, now + 150))
    for changes, error in [({"token": SYSTEM}, 6009), ({"mint": wrong_mint}, 6004), ({"principal": 0}, 6008)]:
        v.send(v.funding(order, **changes), buyer, seller, errors=(error,))
        require(v.account(order.escrow) is None and v.account(order.vault) is None, "failed fund created accounts")
        require(v.amount(ata(buyer.pubkey(), mint)) == PRINCIPAL, "failed fund debited buyer")
    opening_sol = v.account(buyer.pubkey())["lamports"]
    fund_signature = v.send([order.fund(v.env, v.program, v.config)], buyer, seller)
    funded_account = v.account(order.escrow)
    funded_vault = v.account(order.vault)
    tx = v.transaction(fund_signature)
    require(tx is not None and tx["meta"]["err"] is None, "funded transaction metadata")
    rent = v.rpc("getMinimumBalanceForRentExemption", 698)
    require(funded_account["lamports"] == rent, "escrow rent exemption")
    require(opening_sol - v.account(buyer.pubkey())["lamports"] == rent + funded_vault["lamports"] + tx["meta"]["fee"],
            "unexpected fee or rent delta")
    require(v.amount(order.vault) == PRINCIPAL and v.amount(ata(buyer.pubkey(), mint)) == 0, "fund token deltas")
    v.send(v.funding(order), buyer, seller, errors=(6001, 6006))
    require(v.amount(order.vault) == PRINCIPAL, "duplicate deposit")
    v.dust(dust_key, order)
    delivery, verdict = secrets.token_bytes(32), secrets.token_bytes(32)
    commit_sig = v.send([order.deliver(v.program, delivery)], v.admin, seller)
    release = order.terminal(v.program, caller=verifier.pubkey(), release=True, delivery_id=delivery, verdict_id=verdict)
    bad_signer = order.terminal(v.program, caller=seller.pubkey(), release=True, delivery_id=delivery, verdict_id=verdict)
    v.send([bad_signer], v.admin, seller, errors=(6000,))
    beneficiary = ata(seller.pubkey(), mint)
    freeze = [meta(beneficiary, write=True), meta(mint), meta(v.authority.pubkey(), True)]
    v.send([token_ix(bytes([10]), *freeze)], v.admin, v.authority)
    before = v.data(order.escrow)
    frozen_sig = v.send([release], v.admin, verifier, errors=(17,))
    require(v.data(order.escrow) == before and before[488] == 1 and v.amount(beneficiary) == 0
            and v.amount(order.vault) == PRINCIPAL + 1, "frozen destination changed an escrow or token balance")
    v.send([token_ix(bytes([11]), *freeze)], v.admin, v.authority)
    v.close(seller)
    require(v.account(beneficiary) is None, "seller ATA still exists")
    refund = order.terminal(v.program, caller=v.admin.pubkey())
    with ThreadPoolExecutor(max_workers=2) as pool:
        payout = pool.submit(v.send, [create_ata(v.admin.pubkey(), seller.pubkey(), mint), release], v.admin, verifier)
        ineligible = pool.submit(v.send, [refund], None, errors=(6002, 6006))
        payout_sig = payout.result()
        ineligible.result()
    replays = [
        v.send([release], v.admin, verifier, errors=(6006,)),
        v.send([refund], errors=(6006,)),
        v.send([order.fund(v.env, v.program, v.config)], buyer, seller, errors=(6006,)),
    ]
    for replay in replays:
        inner = v.transaction(replay)["meta"]["innerInstructions"]
        require(inner == [], "terminal replay performed CPI")
    require(v.amount(order.vault) == 1 and v.amount(beneficiary) == PRINCIPAL and v.data(order.escrow)[488] == 2,
            "payout dust or replay invariant")
    v.evidence["payout"] = dict(escrow=str(order.escrow), vault=str(order.vault), rent_lamports=rent,
                                 fund=fund_signature, commit_delivery=commit_sig, release=payout_sig,
                                 frozen_failure=frozen_sig, fund_slot=tx["slot"], principal=PRINCIPAL, vault_dust=1)
    parts = v.parties(PRINCIPAL)
    buyer, seller, _ = parts
    now = v.now()
    order = v.order(parts, (now + 12, now + 15, now + 22))
    v.fund(order)
    v.dust(dust_key, order)
    v.close(buyer)
    require(v.account(ata(buyer.pubkey(), mint)) is None, "buyer ATA still exists")
    v.wait_clock(order.deliver_by)
    refund = order.terminal(v.program, caller=v.admin.pubkey())
    ref_sig = v.send([create_ata(v.admin.pubkey(), buyer.pubkey(), mint), refund])
    v.send([refund], errors=(6006,))
    require(v.amount(order.vault) == 1 and v.amount(ata(buyer.pubkey(), mint)) == PRINCIPAL
            and v.amount(ata(seller.pubkey(), mint)) == 0 and v.data(order.escrow)[488] == 3,
            "refund dust or replay invariant")
    v.evidence["refund"] = dict(escrow=str(order.escrow), vault=str(order.vault), refund=ref_sig, principal=PRINCIPAL, vault_dust=1)
    print("T-SOL-01..07,09 and terminal replays passed", flush=True)


def simulated_timestamp(v, signed, before_slot, timestamp, expected_error):
    signed_wire = base64.b64encode(bytes(signed)).decode()
    resp = v.rpc("simulateTransaction", signed_wire, {
        "sigVerify": True, "encoding": "base64", "commitment": "processed", "minContextSlot": before_slot,
        "accounts": {"encoding": "base64", "addresses": [str(CLOCK)]},
    })
    result = resp["value"]
    slot = resp["context"]["slot"]
    # Agave omits post-simulation accounts for failures. Bracket that bank with
    # two Clock reads from the same second before accepting its deadline result.
    clocks = result.get("accounts")
    if clocks and clocks[0]:
        bank_ts = struct.unpack_from("<q", base64.b64decode(clocks[0]["data"][0]), 32)[0]
    else:
        after_slot, bank_ts = v.clock_sample(min_slot=slot)
        require(before_slot <= slot <= after_slot, "simulation escaped sampled bank range")
    if bank_ts != timestamp:
        return None
    if expected_error is None:
        require(result["err"] is None, f"unexpected boundary failure at {timestamp}: {result['err']}")
    else:
        error = result.get("err") or {}
        ix_err = error.get("InstructionError")
        require(isinstance(ix_err, list) and ix_err[1] == {"Custom": expected_error},
                f"wrong deadline error at {timestamp}: {error}")
    return slot


def boundary_attempt(v, attempt):
    participants = v.parties(30)
    buyer, seller, verifier = participants
    target = v.now() + 55
    # Each time point gets independent funded and delivered PDAs, including a
    # dedicated silent-verifier order. The clock advances naturally on this validator.
    cases = {}
    for offset in (-1, 0, 1):
        unfunded = v.order(participants, (target, target + 5, target + 10), 1)
        funded = v.order(participants, (target - 8, target, target + 15), 1)
        v.fund(funded)
        delivered = v.order(participants, (target - 10, target - 5, target), 1)
        silent = v.order(participants, (target - 10, target - 5, target), 1)
        delivery, verdict = secrets.token_bytes(32), secrets.token_bytes(32)
        for case in (delivered, silent):
            v.fund(case)
            v.send([case.deliver(v.program, delivery)], v.admin, seller)
        cases[offset] = unfunded, funded, delivered, silent, delivery, verdict
    v.wait_clock(target - 4)
    signed_cases = {}
    for offset, records in cases.items():
        unfunded, funded, delivered, silent, delivery, verdict = records
        early = offset < 0
        release = delivered.terminal(v.program, caller=verifier.pubkey(), release=True, delivery_id=delivery, verdict_id=verdict)
        ref = delivered.terminal(v.program, caller=v.admin.pubkey())
        signed_cases[offset] = [
            (v.signed(v.funding(unfunded), buyer, seller), None if early else 6002),
            (v.signed([funded.deliver(v.program, delivery)], v.admin, seller), None if early else 6002),
            (v.signed([funded.terminal(v.program, caller=v.admin.pubkey())], v.admin), 6002 if early else None),
            (v.signed([release], v.admin, verifier), None if early else 6002),
            (v.signed([ref], v.admin), 6002 if early else None),
            (v.signed([silent.terminal(v.program, caller=v.admin.pubkey())], v.admin), 6002 if early else None),
        ]
    sampled_slots = {}
    with ThreadPoolExecutor(max_workers=8) as pool:
        for offset in (-1, 0, 1):
            expected_time = target + offset
            v.wait_clock(expected_time)
            before_slot, timestamp = v.clock_sample()
            if timestamp != expected_time:
                print(f"Boundary sampling {attempt}: missed validator second {expected_time}", flush=True)
                return False
            jobs = [pool.submit(simulated_timestamp, v, tx, before_slot, expected_time, error)
                    for tx, error in signed_cases[offset]]
            slots = [job.result() for job in jobs]
            if any(slot is None for slot in slots):
                print(f"Boundary sampling {attempt}: validator second changed during simulation", flush=True)
                return False
            sampled_slots[str(offset)] = slots
            if offset == 0:
                if v.now() != target:
                    return False
                denied = pool.submit(v.submit, signed_cases[0][3][0], (6002, 6006))
                refunded = pool.submit(v.submit, signed_cases[0][4][0])
        refund_sig, late_release_sig = refunded.result(), denied.result()

    _, _, order, _, _, _ = cases[0]
    require(v.data(order.escrow)[488] == 3 and v.amount(order.vault) == 0, "deadline race did not refund once")
    v.evidence["deadline_boundaries"] = dict(clock_mechanism="natural Clock sysvar seconds, processed-bank simulation",
                                             target=target, offsets=[-1, 0, 1], cases_per_offset=6,
                                             simulation_slots=sampled_slots,
                                             late_signed_release=late_release_sig, refund=refund_sig)
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("deployment", type=Path)
    parser.add_argument("keys", type=Path)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    validator = Validator(json.loads(args.deployment.read_text()), args.keys)
    program_cases(validator)
    for attempt in range(1, 4):
        if boundary_attempt(validator, attempt):
            break
    else:
        raise TimeoutError("failed to sample the validator's three exact deadline seconds")
    validator.finalize(validator.signatures)
    print("T-TIME-01..05 passed against the local validator; all submitted transactions finalized", flush=True)
    if args.out:
        args.out.write_text(json.dumps(validator.evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
