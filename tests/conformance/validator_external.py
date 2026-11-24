"""Recover a permissionless v0 refund with an independent caller and fee payer."""

import base64
import hashlib
import json
import os
import signal
import sqlite3
import struct
import subprocess
import time
from contextlib import closing

from solders.address_lookup_table_account import AddressLookupTableAccount
from solders.hash import Hash
from solders.instruction import AccountMeta, Instruction
from solders.keypair import Keypair
from solders.message import MessageV0
from solders.pubkey import Pubkey
from solders.system_program import (
    CreateLookupTableParams,
    ExtendLookupTableParams,
    TransferParams,
    create_lookup_table,
    extend_lookup_table,
    transfer,
)
from solders.transaction import VersionedTransaction

from validator_scenarios import CLI, TOKEN, cli, confirmed_tx, key, rpc, save, submit, verify


def database(session):
    return closing(sqlite3.connect((session / "coordinator.sqlite").as_uri() + "?mode=ro", uri=True))


def fund_and_stop(private, session):
    # Stop this isolated demo and its daemon together before the buyer's own timeout refund.
    command = [CLI, "demo", "--scenario", "no-delivery", "--backend", "local-validator",
               "--deployment", private / "deployment.json", "--output-dir", session]
    with subprocess.Popen(command, start_new_session=True, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True) as demo:
        try:
            deadline = time.monotonic() + 150
            while time.monotonic() < deadline:
                if demo.poll() is not None:
                    raise AssertionError(f"demo exited before funding: {demo.communicate()}")
                if (session / "trust.json").is_file():
                    with database(session) as db:
                        row = db.execute("""
                            SELECT o.order_id,r.signed_bytes,q.signed_bytes,i.local_signature
                            FROM orders o JOIN signed_messages r ON r.message_id=o.request_id
                            JOIN signed_messages q ON q.message_id=o.quote_id
                            JOIN intents i ON i.order_id=o.order_id AND i.action='fund'
                            WHERE o.state='Funded' AND i.state='finalized_success'
                        """).fetchone()
                    if row:
                        order, request, quote, funding = row
                        return order, json.loads(request)["body"], json.loads(quote)["body"], funding
                time.sleep(0.2)
            raise TimeoutError("coordinator did not finalize funding")
        finally:
            if demo.poll() is None:
                os.killpg(demo.pid, signal.SIGTERM)
                demo.communicate(timeout=15)


def token_balances(url, accounts):
    return [
        int(rpc(url, "getTokenAccountBalance", [str(account), {"commitment": "finalized"}])
            ["value"]["amount"])
        for account in accounts
    ]


def wait_for_refund_clock(url, target):
    end = time.monotonic() + 150
    while time.monotonic() < end:
        clock = rpc(url, "getAccountInfo", ["SysvarC1ock11111111111111111111111111111111",
                                            {"encoding": "base64", "commitment": "finalized"}])
        raw = base64.b64decode(clock["value"]["data"][0], validate=True)
        if struct.unpack_from("<q", raw, 32)[0] >= target:
            return
        time.sleep(0.4)
    raise TimeoutError("finalized Clock did not reach deliver_by")


def submit_v0_refund(url, payer, caller, terms, nonce):
    def address(field):
        return Pubkey.from_string(terms[field])

    escrow, vault, buyer_ata, mint = map(address, ("escrow_pda", "vault_token_account",
                                                   "buyer_token_account", "mint"))
    addresses = [escrow, vault, buyer_ata, mint]
    recent_slot = rpc(url, "getSlot", [{"commitment": "finalized"}])
    create, table = create_lookup_table(CreateLookupTableParams(
        authority_address=caller.pubkey(), payer_address=payer.pubkey(), recent_slot=recent_slot))
    extend = extend_lookup_table(ExtendLookupTableParams(
        lookup_table_address=table, authority_address=caller.pubkey(),
        payer_address=payer.pubkey(), new_addresses=addresses))
    give_caller_sol = transfer(TransferParams(from_pubkey=payer.pubkey(),
                                              to_pubkey=caller.pubkey(), lamports=2_000_000))
    submit(url, [give_caller_sol, create, extend], payer, [caller])

    refund = Instruction(address("program_id"), hashlib.sha256(b"global:refund").digest()[:8] + nonce, [
        AccountMeta(caller.pubkey(), True, False), AccountMeta(escrow, False, True),
        AccountMeta(mint, False, False), AccountMeta(vault, False, True),
        AccountMeta(buyer_ata, False, True), AccountMeta(TOKEN, False, False),
    ])
    blockhash = rpc(url, "getLatestBlockhash", [{"commitment": "confirmed"}])["value"]["blockhash"]
    message = MessageV0.try_compile(payer.pubkey(), [refund],
                                    [AddressLookupTableAccount(table, addresses)],
                                    Hash.from_string(blockhash))
    assert len(message.address_table_lookups) == 1
    tx = VersionedTransaction(message, [payer, caller])
    sig = rpc(url, "sendTransaction", [base64.b64encode(bytes(tx)).decode(),
                                       {"encoding": "base64", "preflightCommitment": "confirmed"}])
    assert sig == str(tx.signatures[0])
    record = confirmed_tx(url, sig)
    assert record["version"] == 0 and len(record["transaction"]["signatures"]) == 2
    assert record["transaction"]["message"]["accountKeys"][:2] == [str(payer.pubkey()), str(caller.pubkey())]
    loaded = record["meta"]["loadedAddresses"]
    assert set(loaded["writable"]) == {str(escrow), str(vault), str(buyer_ata)}
    assert loaded["readonly"] == [str(mint)]
    return sig, record["slot"], str(table)


def permissionless_refund(private, dep, public):
    session = private / "external-session"
    url = dep["rpc_url"]
    order, request, quote, funding = fund_and_stop(private, session)
    terms = quote["settlement"]
    accounts = [terms[k] for k in ("buyer_token_account", "seller_token_account", "vault_token_account")]
    before = token_balances(url, accounts)
    assert before == [3_000_000, 0, 2_000_000]
    caller, payer = Keypair(), key(private, "fee-payer")
    parties = {key(private, name).pubkey() for name in ("buyer", "seller", "verifier", "coordinator")}
    assert caller.pubkey() != payer.pubkey() and parties.isdisjoint((caller.pubkey(), payer.pubkey()))
    wait_for_refund_clock(url, int(quote["deadlines"]["deliver_by"]))
    signature, slot, table = submit_v0_refund(url, payer, caller, terms, bytes.fromhex(request["nonce"]))

    snapshots = []
    receipt = None
    for _ in range(2):
        # Without --endpoint, each CLI invocation launches a new coordinator on the existing DB.
        view = json.loads(cli("order", "inspect", order, "--trust-file",
                          session / "trust.json", "--json").stdout)
        assert view["state"] == "Refunded" and (view["held"], view["refunded"]) == ("0", "2000000")
        with database(session) as db:
            [(bundle,)] = db.execute("SELECT bundle FROM receipts WHERE order_id=?", (order,)).fetchall()
            [(observed, height, state)] = db.execute("""
                SELECT local_signature,last_valid_block_height,state FROM intents
                WHERE order_id=? AND action='refund'
            """, (order,)).fetchall()
            assert (observed, height, state) == (signature, None, "finalized_success")
        assert receipt is None or receipt == bundle
        receipt = bundle
        exported = json.loads(bundle)
        assert "verdict" not in exported and "delivery" not in exported
        assert exported["receipt"]["body"]["outcome"] == "refunded_timeout_before_delivery"
        ledger = exported["ledger"]
        assert ledger["funding_signature"] == funding and ledger["terminal_signature"] == signature
        assert ledger["finalized_slot"] == str(slot) and ledger["terminal_instruction"] == "refund"
        (session / "receipt.json").write_bytes(bundle)
        verify(session, url)
        after = token_balances(url, accounts)
        assert after == [5_000_000, 0, 0]
        snapshots.append(after)

    result = dict(order_id=order, funding_signature=funding, terminal_signature=signature, slot=slot,
                  caller=str(caller.pubkey()), fee_payer=str(payer.pubkey()), transaction_version=0,
                  address_lookup_table=table, funded_balances=before, restart_balances=snapshots)
    save(public / "external-refund.json", result)
    print("VALIDATOR_EXTERNAL " + json.dumps(result, separators=(",", ":")), flush=True)
