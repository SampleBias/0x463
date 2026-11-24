"""Drive the public CLI and compare finalized SPL deltas against the simulator."""
import argparse
import base64
import hashlib
import signal
from concurrent.futures import ThreadPoolExecutor, as_completed
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import sqlite3
import struct
import subprocess
import sys
from threading import Event, Thread
from urllib.request import Request, ProxyHandler, build_opener
from solders.transaction import Transaction
import time
import traceback

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
from validator_fixture import ATA, SYSTEM, TOKEN, AccountMeta, Instruction, Keypair, Pubkey, associated, key, rpc, submit, transfer, TransferParams

CLI = Path("target/debug/settle463").resolve()
CASES = {"pass": ("paid", -2000000, 2000000), "bad-artifact": ("refunded_timeout_after_delivery", 0, 0),
         "no-delivery": ("refunded_timeout_before_delivery", 0, 0), "verifier-offline": ("refunded_timeout_after_delivery", 0, 0)}


def cli(*args, code=0, env=None):
    p = subprocess.run([CLI, *map(str, args)], env=env, capture_output=True, text=True, timeout=250)
    if p.returncode != code:
        raise RuntimeError(f"{args[:4]} exited {p.returncode}, expected {code}\n{p.stdout}\n{p.stderr}")
    return p


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def new_parties(fixture, name, deployment):
    """Give each concurrent order its own five-token buyer and fee-paying wallets."""
    if name == "pass":
        return fixture
    directory = fixture / name
    directory.mkdir(mode=0o700)
    shutil.copyfile(fixture / "deployment.json", directory / "deployment.json")
    shutil.copyfile(fixture / "fee-payer.json", directory / "fee-payer.json")
    payer = key(fixture, "fee-payer")
    authority = key(fixture, "mint-authority")
    mint = Pubkey.from_string(deployment["mint"])
    ixs, parties = [], {}
    for role in ("buyer", "seller", "verifier", "coordinator"):
        kp = Keypair()
        save(directory / (role + ".json"), list(bytes(kp)))
        parties[role] = p = kp.pubkey()
        ixs.append(transfer(TransferParams(from_pubkey=payer.pubkey(), to_pubkey=p, lamports=300_000_000)))
    for role in ("buyer", "seller"):
        p = parties[role]
        ixs.append(Instruction(ATA, b"\x01", [AccountMeta(payer.pubkey(), True, True),
                                              AccountMeta(associated(p, mint), False, True), AccountMeta(
                                                  p, False, False), AccountMeta(mint, False, False),
                                              AccountMeta(SYSTEM, False, False), AccountMeta(TOKEN, False, False)]))
    ixs.append(Instruction(TOKEN, bytes([14]) + struct.pack("<QB", 5000000, 6), [
        AccountMeta(mint, False, True), AccountMeta(associated(parties["buyer"], mint), False, True),
        AccountMeta(authority.pubkey(), True, False)]))
    submit(deployment["rpc_url"], ixs, payer, [authority])
    return directory


def confirmed_tx(url, sig):
    for _ in range(120):
        t = rpc(url, "getTransaction", [sig, {"encoding": "json",
                "commitment": "finalized", "maxSupportedTransactionVersion": 0}])
        if t:
            assert t["meta"]["err"] is None
            return t
        time.sleep(0.5)
    raise RuntimeError(f"expected finalized transaction {sig}")


def verify(session, url, code=0):
    label = {0: "ledger_verified", 2: "invalid_evidence", 5: "unverifiable"}[code]
    out = cli("receipt", "verify", session / "receipt.json", "--rpc",
              url, "--trust-file", session / "trust.json", code=code)
    assert out.stdout.strip() == label, out.stdout


@contextmanager
def proxy_rpc(url, transform, before_forward=None):
    class Proxy(BaseHTTPRequestHandler):
        def do_POST(self):
            raw = self.rfile.read(int(self.headers["Content-Length"]))
            req = json.loads(raw)
            if before_forward and not before_forward(req["method"], req.get("params", [])):
                self.close_connection = True
                return
            upstream = Request(url, data=raw, headers={"Content-Type": "application/json"})
            with build_opener(ProxyHandler({})).open(upstream, timeout=15) as response:
                reply = json.load(response)
            if not transform(req["method"], req.get("params", []), reply):
                self.close_connection = True
                return
            body = json.dumps(reply).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    with ThreadingHTTPServer(("127.0.0.1", 0), Proxy) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}"
        finally:
            server.shutdown()
            thread.join()


def evidence_rpc(url, signature, mode):
    def change(method, params, reply):
        if method == "getTransaction" and params[0] == signature:
            if mode == "missing":
                reply["result"] = None
            else:
                reply["result"]["meta"]["err"] = {"InstructionError": [0, {"Custom": 6005}]}
        if method == "getSignatureStatuses" and signature in params[0] and mode == "missing":
            reply["result"]["value"] = [None] * len(params[0])
        return True
    return proxy_rpc(url, change)


def demo(case, private, dep, public):
    session = private / ("session-" + case)
    sim = private / ("sim-" + case)
    args = ["demo", "--scenario", case]
    model = cli(*args, "--backend", "sim", "--output-dir", sim)
    expected = dict(line.split("=", 1) for line in model.stdout.splitlines())
    run = cli(*args, "--backend", "local-validator", "--deployment",
              private / "deployment.json", "--output-dir", session)
    fields = dict(line.split("=", 1) for line in run.stdout.splitlines())
    assert list(fields) == list(
        expected) and fields["backend"] == "local-validator" and fields["asset"] == dep["mint"]
    assert tuple(fields[k] for k in ("outcome", "delta_buyer",
                 "delta_seller")) == tuple(map(str, CASES[case]))
    assert all(fields[k] == expected[k]
               for k in ("outcome", "principal", "delta_buyer", "delta_seller", "delta_escrow"))
    bundle = json.loads((session / "receipt.json").read_bytes())
    model_bundle = json.loads((sim / "receipt.json").read_bytes())
    assert all((k in bundle) == (k in model_bundle) for k in ("delivery", "verdict"))
    verify(session, dep["rpc_url"])
    off = cli("receipt", "verify", session / "receipt.json",
              "--offline", "--trust-file", session / "trust.json")
    assert off.stdout.strip() == "evidence_signed"
    ledger = bundle["ledger"]
    quote = bundle["quote"]["body"]["settlement"]
    url = dep["rpc_url"]
    assert ledger["asset_label"] == "local validator test asset"
    assert f'local validator test asset mint={dep["mint"]}' in run.stderr
    txs = {k: confirmed_tx(url, ledger[k]) for k in ("funding_signature",
                                                     "delivery_signature", "terminal_signature") if k in ledger}
    assert str(txs["terminal_signature"]["slot"]) == ledger["finalized_slot"]
    vault = rpc(url, "getTokenAccountBalance", [ledger["vault"],
                {"commitment": "finalized"}])["value"]["amount"]
    assert int(vault) == 0
    pda = rpc(url, "getAccountInfo", [ledger["order_pda"], {
              "encoding": "base64", "commitment": "finalized"}])["value"]
    data = base64.b64decode(pda["data"][0])
    assert pda["owner"] == dep["program_id"] and len(data) == 698
    order_pda, bump = Pubkey.find_program_address([b"settle463", b"escrow", b"v1", bytes(key(private, "buyer").pubkey()),
                                                   bytes.fromhex(bundle["request"]["body"]["nonce"])], Pubkey.from_string(dep["program_id"]))
    assert str(order_pda) == ledger["order_pda"]
    rent = rpc(url, "getMinimumBalanceForRentExemption", [len(data)])
    assert pda["lamports"] >= rent
    if case == "pass":
        assert "buyer_download length=52 sha256=37ba5c2e" in run.stderr
        contacted = Event()

        def trap_endpoint(method, params, reply):
            contacted.set()
            return True
        with proxy_rpc(url, trap_endpoint) as unapproved:
            hinted = {**bundle, "rpc_url": unapproved}
            save(session / "hint.json", hinted)
            cli("receipt", "verify", session / "hint.json", "--trust-file", session / "trust.json", code=1)
            trusted = cli("receipt", "verify", session / "hint.json", "--rpc",
                          url, "--trust-file", session / "trust.json")
            assert trusted.stdout.strip() == "ledger_verified" and not contacted.is_set()
        for mode, exit_code in (("missing", 5), ("failed", 2)):
            with evidence_rpc(url, ledger["terminal_signature"], mode) as proxy:
                verify(session, proxy, exit_code)
        wrong_pin = cli("receipt", "verify", session / "receipt.json", "--rpc", url,
                        "--trust-file", session / "trust.json", "--program-id", str(SYSTEM), code=2)
        assert wrong_pin.stdout.strip() == "invalid_evidence"
        changed = json.loads((session / "receipt.json").read_bytes())
        changed["ledger"]["program_id"] = str(SYSTEM)
        save(session / "tampered.json", changed)
        assert cli("receipt", "verify", session / "tampered.json", "--rpc", url, "--trust-file",
                   session / "trust.json", code=2).stdout.strip() == "invalid_evidence"
    dest = public / case
    dest.mkdir()
    for n in ("receipt.json", "trust.json"):
        shutil.copyfile(session / n, dest / n)
    (dest / "demo.stdout").write_text(run.stdout)
    info = dict(scenario=case, **fields, ledger=ledger, slots={k: t["slot"] for k, t in txs.items()},
                escrow_bump=bump, rent_exempt_lamports=rent, buyer_ata=quote["buyer_token_account"], seller_ata=quote["seller_token_account"])
    save(dest / "evidence.json", info)
    print("VALIDATOR_CASE " + json.dumps(info, separators=(",", ":")), flush=True)


def recovery(action, private, dep, public, *, before_broadcast=False):
    directory = private / "interrupted"
    discriminator = hashlib.sha256(("global:" + action).encode()).digest()[:8]
    program = Pubkey.from_string(dep["program_id"])
    lost = Event()
    interrupted = {}

    def interrupt(method, params, accepted=None):
        if method != "sendTransaction" or lost.is_set():
            return True
        tx = Transaction.from_bytes(base64.b64decode(params[0]))
        if not any(tx.message.account_keys[ix.program_id_index] == program and
                   bytes(ix.data).startswith(discriminator) for ix in tx.message.instructions):
            return True
        signature = str(tx.signatures[0])
        if accepted is not None:
            assert signature == accepted
        children = Path(f"/proc/{process.pid}/task/{process.pid}/children").read_text().split()
        servers = [int(c) for c in children if b"__serve" in Path(
            f"/proc/{c}/cmdline").read_bytes().split(b"\x00")]
        assert len(servers) == 1, servers
        interrupted["signature"] = signature
        os.kill(servers[0], signal.SIGKILL)
        lost.set()
        return False

    def lose_response(method, params, reply):
        if method == "getTransaction" and params[0] == interrupted.get("hide_funding"):
            reply["result"] = None
        if action == "release" and method == "getTransaction" and reply.get("result"):
            reply["result"]["blockTime"] = None
        return before_broadcast or "result" not in reply or interrupt(method, params, reply["result"])

    with proxy_rpc(dep["rpc_url"], lose_response, interrupt if before_broadcast else None) as proxy:
        save(private / "deployment.json", {**dep, "rpc_url": proxy})
        with subprocess.Popen([str(CLI), "demo", "--scenario", "pass", "--backend", "local-validator",
                               "--deployment", str(private / "deployment.json"), "--output-dir", str(directory)],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
            stdout, stderr = process.communicate(timeout=250)
            assert lost.is_set() and process.returncode != 0, (action, stdout, stderr)
        if before_broadcast:
            assert rpc(dep["rpc_url"], "getSignatureStatuses", [[interrupted["signature"]],
                       {"searchTransactionHistory": True}])["value"] == [None]
        elif action == "release":
            confirmed_tx(dep["rpc_url"], interrupted["signature"])
            with sqlite3.connect(directory / "coordinator.sqlite") as db:
                order, funding = db.execute(
                    "select order_id,local_signature from intents where action='fund'").fetchone()
            interrupted["hide_funding"] = funding
            cli("order", "inspect", order, "--trust-file", directory / "trust.json", "--json")
            with sqlite3.connect(directory / "coordinator.sqlite") as db:
                assert db.execute("select count(*) from receipts").fetchone()[0] == 0
                assert db.execute(
                    "select count(*) from audit_events where event_type='reconciliation_gap'").fetchone()[0] >= 1
            del interrupted["hide_funding"]
        result = recovered(action, directory, private, dep, interrupted["signature"], before_broadcast)
    result["interruption"] = "before_broadcast" if before_broadcast else "lost_response"
    save(public / ("recovery-" + result["interruption"] + "-" + action + ".json"), result)
    print("VALIDATOR_RECOVERY " + json.dumps(result, separators=(",", ":")), flush=True)


def recovered(action, directory, private, dep, accepted_signature, before_broadcast):
    trust = json.loads((directory / "trust.json").read_bytes())
    order = trust["trusted_order"]
    with sqlite3.connect(directory / "coordinator.sqlite") as db:
        sig, raw = db.execute(
            "select local_signature,signed_bytes from intents where action=? and order_id=?", (action, order)).fetchone()
        assert sig == accepted_signature and len(raw) > 64 and db.execute(
            "select count(*) from receipts").fetchone()[0] == 0
    if not before_broadcast:
        confirmed_tx(dep["rpc_url"], sig)
    buyer = associated(key(private, "buyer").pubkey(), Pubkey.from_string(dep["mint"]))
    seller = associated(key(private, "seller").pubkey(), Pubkey.from_string(dep["mint"]))

    def balance(account):
        return int(rpc(dep["rpc_url"], "getTokenAccountBalance", [str(account), {"commitment": "finalized"}])["value"]["amount"])
    snapshots = []
    for _ in range(2):
        for attempt in range(24):
            view = json.loads(cli("order", "inspect", order, "--trust-file",
                              directory / "trust.json", "--json").stdout)
            if view["state"] == ("Funded" if action == "fund" else "Paid"):
                break
            time.sleep(1)
        else:
            raise AssertionError(view)
        with sqlite3.connect(directory / "coordinator.sqlite") as db:
            receipts = db.execute("select count(*) from receipts").fetchone()[0]
            assert db.execute(
                "select count(*) from intents where action=? and state='finalized_success'", (action,)).fetchone()[0] == 1
            if action == "release":
                (directory /
                 "receipt.json").write_bytes(db.execute("select bundle from receipts").fetchone()[0])
        snapshots.append([balance(buyer), balance(seller), int(view["held"]), receipts])
    assert snapshots == [[3000000, 0, 2000000, 0] if action == "fund" else [3000000, 2000000, 0, 1]] * 2
    if action == "release":
        verify(directory, dep["rpc_url"])
    slot = confirmed_tx(dep["rpc_url"], sig)["slot"]
    return dict(action=action, signature=sig, slot=slot, order_id=order, restart_snapshots=snapshots)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    public = parser.parse_args().output.resolve()
    public.mkdir(parents=True)
    fixture = Path(os.environ["PHASE_D_FIXTURE"])
    dep = json.loads((fixture / "deployment.json").read_bytes())
    save(public / "deployment.json", dep)
    commands = [*CASES, "recover-fund", "recover-release",
                "before-release", "adapter-recovery", "external-refund"]
    with ThreadPoolExecutor(max_workers=len(commands) + 1) as pool:
        def run(name):
            private = new_parties(fixture, name, dep)
            if name == "adapter-recovery":
                subprocess.run(["cargo", "test", "--locked", "--offline", "-p", "settle463-solana",
                                "--test", "validator_recovery", "--", "--ignored", "--nocapture"],
                               env=dict(os.environ, PHASE_D_ADAPTER_FIXTURE=str(private)), check=True)
            elif name in CASES:
                demo(name, private, dep, public)
            elif name == "external-refund":
                from validator_external import permissionless_refund
                permissionless_refund(private, dep, public)
            elif name == "before-release":
                recovery("release", private, dep, public, before_broadcast=True)
            else:
                recovery(name.removeprefix("recover-"), private, dep, public)
        tasks = {pool.submit(run, name): name for name in commands}
        program = pool.submit(subprocess.run, [sys.executable, "programs/settle463-escrow/tests/validator.py",
                                               fixture / "deployment.json", fixture, "--out", public / "program-tests.json"], check=True)
        tasks[program] = "escrow program"
        failures = []
        for task in as_completed(tasks):
            try:
                task.result()
            except Exception as error:
                failures.append(tasks[task])
                print(f"Validator case failed: {tasks[task]}", file=sys.stderr, flush=True)
                traceback.print_exception(error)
        if failures:
            raise RuntimeError("failed validator cases: " + ", ".join(failures))
    print("VALIDATOR_PROGRAM " + json.dumps(json.loads((public / "program-tests.json").read_text()),
          separators=(",", ":")), flush=True)


if __name__ == "__main__":
    main()
