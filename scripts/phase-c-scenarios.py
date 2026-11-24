"""Exercise the four public CLI flows and retain their public evidence."""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

REFERENCE = "37ba5c2e570e4e256af7049d0d9c27f70b50f5de4210101d7c0bd2b497db7587"
CASES = {
    "pass": ("paid", -2000000, 2000000),
    "bad-artifact": ("refunded_timeout_after_delivery", 0, 0),
    "no-delivery": ("refunded_timeout_before_delivery", 0, 0),
    "verifier-offline": ("refunded_timeout_after_delivery", 0, 0),
}


def require(test, message):
    if not test:
        raise ValueError(message)


def cli(*args):
    result = subprocess.run(["cargo", "run", "--quiet", "--locked", "-p", "settle463", "--", *map(str, args)],
                            capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"{args[0]} exited {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new directory for public receipts, trust inputs and transcripts")
    out = parser.parse_args().output.resolve()
    out.mkdir(parents=True)
    orders = set()
    with tempfile.TemporaryDirectory(prefix="settle463-cli-") as scratch:
        for scenario, (outcome, buyer, seller) in CASES.items():
            session = Path(scratch) / scenario
            run = cli("demo", "--backend", "sim", "--scenario", scenario, "--output-dir", session)
            lines = run.stdout.splitlines()
            require(len(lines) == 9, f"{scenario} stdout: {lines}")
            require(re.fullmatch(r"order_id=[0-9a-f]{64}", lines[0]), lines[0])
            order = lines[0].split("=")[1]
            require(order not in orders, "reused scenario order")
            orders.add(order)
            require(lines[1:8] == ["backend=sim", f"outcome={outcome}", "asset=test-six-decimal", "principal=2000000",
                                    f"delta_buyer={buyer}", f"delta_seller={seller}", "delta_escrow=0"], str(lines))
            receipt = session / "receipt.json"
            trust = session / "trust.json"
            require(lines[8] == f"receipt={receipt}", lines[8])
            trusted = json.loads(trust.read_bytes())
            require(set(trusted) == {"environment", "trusted_order", "trusted_issuer"} and trusted["trusted_order"] == order, "trust file")
            bundle = json.loads(receipt.read_bytes())
            require(bundle["receipt"]["body"]["outcome"] == outcome and bundle["ledger"]["finality"] == "simulated-final", scenario)
            if scenario == "pass":
                require(REFERENCE in run.stderr and "52" in run.stderr, "missing reference download evidence")
                require(bundle["verdict"]["body"]["decision"] == "PASS", "payment verdict")
            if scenario == "bad-artifact":
                v = bundle["verdict"]["body"]
                require(v["decision"] == "FAIL" and "DIGEST_MISMATCH" in v["reason_codes"], "mutation verdict")
            if scenario == "no-delivery":
                require("delivery" not in bundle and "verdict" not in bundle, "unexpected delivery or verdict")
            if scenario == "verifier-offline":
                require("delivery" in bundle and "verdict" not in bundle, "offline verifier evidence")

            verify = cli("receipt", "verify", receipt, "--offline", "--trust-file", trust)
            require("evidence_signed" in verify.stdout and "ledger_verified" not in verify.stdout, verify.stdout)
            inspect = cli("order", "inspect", order, "--trust-file", trust)
            destination = out / scenario
            destination.mkdir()
            shutil.copyfile(receipt, destination / receipt.name)
            shutil.copyfile(trust, destination / trust.name)
            (destination / "demo.stdout").write_text(run.stdout)
            (destination / "verify.stdout").write_text(verify.stdout)
            (destination / "inspect.stdout").write_text(inspect.stdout)
            print(f"scenario={scenario}\n{run.stdout}verify_exit=0\ninspect_exit=0", flush=True)
            if scenario == "pass":
                print(f"buyer_download_length=52 buyer_download_sha256={REFERENCE}", flush=True)


if __name__ == "__main__":
    main()
