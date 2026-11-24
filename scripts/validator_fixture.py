"""Create a classic SPL mint and initialize the escrow config on a fresh loopback validator."""
import base64
import hashlib
import json
from pathlib import Path
import random
import socket
import subprocess
import sys
import time
from urllib.parse import urlparse
from urllib.request import ProxyHandler, Request, build_opener

from solders.hash import Hash
from solders.instruction import AccountMeta, Instruction
from solders.keypair import Keypair
from solders.pubkey import Pubkey
from solders.system_program import TransferParams, transfer
from solders.transaction import Transaction

TOKEN = Pubkey.from_string("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
ATA = Pubkey.from_string("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
LOADER = Pubkey.from_string("BPFLoaderUpgradeab1e11111111111111111111111")
SYSTEM = Pubkey.default()


def available_port():
    for _ in range(100):
        base = random.SystemRandom().randrange(21000, 49000)
        sockets = []
        try:
            for port in range(base, base + 3):
                s = socket.socket()
                sockets.append(s)
                s.bind(("127.0.0.1", port))
            return base
        except OSError:
            continue
        finally:
            for s in sockets:
                s.close()
    raise RuntimeError("no free consecutive ports")


def rpc(url, method, params=()):
    if urlparse(url).hostname != "127.0.0.1":
        raise ValueError("the fixture requires a loopback RPC")
    req = Request(url, data=json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params)).encode(),
                  headers={"Content-Type": "application/json"})
    with build_opener(ProxyHandler({})).open(req, timeout=10) as response:
        message = json.load(response)
    if "error" in message:
        raise RuntimeError(f"{method}: {message['error']}")
    return message["result"]


def key(directory, name):
    return Keypair.from_bytes(bytes(json.loads((directory / (name + ".json")).read_text())))


def associated(owner, mint):
    return Pubkey.find_program_address([bytes(owner), bytes(TOKEN), bytes(mint)], ATA)[0]


def submit(url, instructions, payer, signers=()):
    latest = rpc(url, "getLatestBlockhash", [{"commitment": "confirmed"}])["value"]
    tx = Transaction.new_signed_with_payer(instructions, payer.pubkey(), [payer, *signers],
                                           Hash.from_string(latest["blockhash"]))
    sig = rpc(url, "sendTransaction", [base64.b64encode(bytes(tx)).decode(),
                                       {"encoding": "base64", "preflightCommitment": "confirmed"}])
    if sig != str(tx.signatures[0]):
        raise RuntimeError("RPC returned a different transaction signature")
    for _ in range(160):
        status = rpc(url, "getSignatureStatuses", [[sig], {"searchTransactionHistory": True}])["value"][0]
        if status and status["err"] is not None:
            raise RuntimeError(f"{sig}: {status['err']}")
        if status and status["confirmationStatus"] == "finalized":
            return sig, status["slot"]
        time.sleep(0.5)
    raise RuntimeError(f"timed out waiting for finalized signature {sig}")


def prepare(directory, url, program):
    payer = key(directory, "fee-payer")
    buyer, seller, verifier, issuer, authority = [key(directory, name).pubkey() for name in
                                                  ("buyer", "seller", "verifier", "coordinator", "mint-authority")]
    mint = key(directory, "mint").pubkey()
    genesis = rpc(url, "getGenesisHash")
    config = Pubkey.find_program_address([b"settle463", b"config", b"v1"], program)[0]
    program_data = Pubkey.find_program_address([bytes(program)], LOADER)[0]
    submit(url, [transfer(TransferParams(from_pubkey=payer.pubkey(), to_pubkey=p, lamports=300_000_000))
                 for p in [buyer, seller, verifier, issuer, authority]], payer)
    cfg = directory / "cli.yml"
    subprocess.run(["solana", "--config", str(cfg), "config", "set", "--url", url, "--keypair",
                    str(directory / "fee-payer.json"), "--commitment", "finalized"], check=True)
    spl = ["spl-token", "--config", str(cfg), "--url", url, "--program-id", str(TOKEN),
           "--fee-payer", str(directory / "fee-payer.json")]
    subprocess.run(["spl-token", "--version"], check=True)
    subprocess.run(spl + ["create-token", "--decimals", "6", "--enable-freeze", "--mint-authority",
                          str(authority), str(directory / "mint.json")], check=True)
    for owner in [buyer, seller]:
        subprocess.run(spl + ["create-account", str(mint), "--owner", str(owner)], check=True)
    buyer_ata = associated(buyer, mint)
    subprocess.run(spl + ["mint", str(mint), "5", str(buyer_ata), "--mint-authority",
                          str(directory / "mint-authority.json")], check=True)
    m = rpc(url, "getAccountInfo", [str(mint), {
            "encoding": "jsonParsed", "commitment": "finalized"}])["value"]
    if m["owner"] != str(TOKEN) or m["data"]["parsed"]["info"]["decimals"] != 6 or \
            m["data"]["parsed"]["info"]["freezeAuthority"] != str(authority):
        raise RuntimeError("wrong mint owner, decimals or freeze authority")
    if rpc(url, "getTokenAccountBalance", [str(buyer_ata), {"commitment": "finalized"}])["value"]["amount"] != "5000000":
        raise RuntimeError("wrong buyer opening balance")
    init = Instruction(program, hashlib.sha256(b"global:initialize_config").digest()[:8] + bytes(Hash.from_string(genesis)),
                       [AccountMeta(payer.pubkey(), True, True), AccountMeta(config, False, True),
                        AccountMeta(program_data, False, False), AccountMeta(mint, False, False),
                        AccountMeta(TOKEN, False, False), AccountMeta(SYSTEM, False, False)])
    signature, slot = submit(url, [init], payer)
    obj = dict(rpc_url=url, genesis=genesis, program_id=str(program), config=str(config),
               mint=str(mint), token_program=str(TOKEN))
    (directory / "deployment.json").write_text(json.dumps(obj, indent=2) + "\n")
    print(f"initialize_config={signature} slot={slot} deployment={json.dumps(obj)}", flush=True)


if __name__ == "__main__":
    if sys.argv[1:] == ["port"]:
        print(available_port())
    else:
        command, folder, rpc_url, program_id = sys.argv[1:]
        if command != "prepare":
            raise SystemExit("expected prepare")
        prepare(Path(folder), rpc_url, Pubkey.from_string(program_id))
