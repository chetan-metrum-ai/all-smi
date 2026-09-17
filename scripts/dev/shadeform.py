#!/usr/bin/env python3
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
"""Shadeform GPU instance lifecycle for all-smi deep-telemetry development.

Loads SHADEFORM_API_KEY from env.json in-process and sends it only as the
X-API-KEY header. Never prints the key, never passes it on a CLI argv.

Subcommands:
  create   Rent one H100 (fallback H200). Refuse if state already has a live id.
  status   Print instance status from .shadeform-state.json + API.
  ssh-cmd  Run a remote command over SSH using the project key.
  delete   Tear down the instance (requires --yes-i-am-sure).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

BASE_URL = "https://api.shadeform.ai/v1"
REPO_ROOT = Path(__file__).resolve().parents[2]
ENV_PATH = REPO_ROOT / "env.json"
STATE_PATH = REPO_ROOT / ".shadeform-state.json"
SSH_KEY_PATH = REPO_ROOT / ".shadeform_ed25519"
SSH_PUB_PATH = REPO_ROOT / ".shadeform_ed25519.pub"
INSTANCE_NAME = "metrum-allsmi-dev"
BANNED_GPU = {"B200", "b200"}


def load_api_key() -> str:
    if not ENV_PATH.is_file():
        sys.exit(f"missing {ENV_PATH}; expected {{\"SHADEFORM_API_KEY\": \"...\"}}")
    with ENV_PATH.open("r", encoding="utf-8") as f:
        data = json.load(f)
    key = data.get("SHADEFORM_API_KEY")
    if not isinstance(key, str) or not key.strip():
        sys.exit("env.json has no non-empty SHADEFORM_API_KEY")
    return key.strip()


def api_request(
    method: str,
    path: str,
    api_key: str,
    body: dict[str, Any] | None = None,
) -> Any:
    url = f"{BASE_URL}{path}"
    data = None
    headers = {
        "X-API-KEY": api_key,
        "Accept": "application/json",
    }
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            raw = resp.read().decode("utf-8")
            if not raw:
                return None
            return json.loads(raw)
    except urllib.error.HTTPError as e:
        err_body = e.read().decode("utf-8", errors="replace")
        sys.exit(f"Shadeform API {method} {path} failed: HTTP {e.code}: {err_body}")
    except urllib.error.URLError as e:
        sys.exit(f"Shadeform API {method} {path} failed: {e}")


def load_state() -> dict[str, Any] | None:
    if not STATE_PATH.is_file():
        return None
    with STATE_PATH.open("r", encoding="utf-8") as f:
        return json.load(f)


def save_state(state: dict[str, Any]) -> None:
    with STATE_PATH.open("w", encoding="utf-8") as f:
        json.dump(state, f, indent=2)
        f.write("\n")
    # Restrict permissions; may contain IPs and instance ids only, still private.
    os.chmod(STATE_PATH, 0o600)


def clear_state() -> None:
    if STATE_PATH.is_file():
        STATE_PATH.unlink()


def ensure_ssh_key(api_key: str) -> str:
    """Register the project public key if needed; return Shadeform ssh_key_id (UUID)."""
    if not SSH_PUB_PATH.is_file():
        sys.exit(
            f"missing {SSH_PUB_PATH}; generate with: "
            "ssh-keygen -t ed25519 -f .shadeform_ed25519 -N '' -C metrum-allsmi-dev"
        )
    pub = SSH_PUB_PATH.read_text(encoding="utf-8").strip()
    keys_resp = api_request("GET", "/sshkeys", api_key)
    existing = (
        keys_resp
        if isinstance(keys_resp, list)
        else (keys_resp or {}).get("ssh_keys") or (keys_resp or {}).get("data") or []
    )
    for entry in existing:
        if not isinstance(entry, dict):
            continue
        remote = (entry.get("public_key") or entry.get("key") or "").strip()
        if remote == pub or pub in remote or remote in pub:
            key_id = entry.get("id")
            if not key_id:
                sys.exit("matching ssh key has no id")
            return str(key_id)
    name = "metrum-allsmi-dev"
    payload = {"name": name, "public_key": pub}
    added = api_request("POST", "/sshkeys/add", api_key, payload)
    if not isinstance(added, dict) or not added.get("id"):
        sys.exit(f"sshkeys/add did not return id: {added!r}")
    return str(added["id"])


def _os_is_ubuntu_cuda(os_name: str) -> bool:
    """Prefer Ubuntu 22.04 CUDA images; accept Ubuntu 24.04 CUDA as fallback."""
    s = os_name.lower()
    if "ubuntu" not in s:
        return False
    if "cuda" not in s:
        return False
    return "22.04" in s or "2204" in s or "24.04" in s or "2404" in s


def _os_preference(os_name: str) -> int:
    s = os_name.lower()
    if "22.04" in s or "2204" in s:
        return 0
    if "24.04" in s or "2404" in s:
        return 1
    return 2


def pick_instance_type(types: list[dict[str, Any]], prefer: str) -> dict[str, Any]:
    """Pick cheapest available 1-GPU instance for prefer (H100) then H200.

    Shadeform `/instances/types` returns `availability` as a list of
    `{region, available, ...}` and `configuration.os_options` for images.
    `hourly_price` is in US cents.
    """

    def candidates(gpu: str) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        for t in types:
            gpu_type = str(t.get("gpu_type") or "")
            if gpu_type.upper() != gpu.upper():
                continue
            if "B200" in gpu_type.upper() or gpu_type.upper() in BANNED_GPU:
                continue
            num = t.get("num_gpus")
            if num is None:
                num = (t.get("configuration") or {}).get("num_gpus")
            if num is not None and int(num) != 1:
                continue
            cloud = t.get("cloud")
            shade_type = t.get("shade_instance_type")
            if not cloud or not shade_type:
                continue
            cfg = t.get("configuration") or {}
            os_options = cfg.get("os_options") or t.get("os_options") or []
            if isinstance(os_options, str):
                os_options = [os_options]
            os_ok = [str(o) for o in os_options if _os_is_ubuntu_cuda(str(o))]
            if not os_ok:
                continue
            os_ok.sort(key=_os_preference)
            chosen_os = os_ok[0]
            price = t.get("hourly_price")
            try:
                price_f = float(price) if price is not None else float("inf")
            except (TypeError, ValueError):
                price_f = float("inf")
            for avail in t.get("availability") or []:
                if not isinstance(avail, dict) or not avail.get("available"):
                    continue
                region = avail.get("region")
                if not region:
                    continue
                out.append(
                    {
                        "cloud": cloud,
                        "region": region,
                        "shade_instance_type": shade_type,
                        "os": chosen_os,
                        "gpu_type": gpu_type,
                        "hourly_price": None if price_f == float("inf") else price_f,
                        "hourly_price_usd": None
                        if price_f == float("inf")
                        else round(price_f / 100.0, 4),
                        "display_name": avail.get("display_name"),
                    }
                )
        out.sort(
            key=lambda x: (
                x["hourly_price"] is None,
                x["hourly_price"] or 0.0,
                _os_preference(x["os"]),
            )
        )
        return out

    for gpu in (prefer, "H200"):
        found = candidates(gpu)
        if found:
            return found[0]
    sys.exit(
        f"no available 1-GPU H100/H200 Ubuntu CUDA instance types "
        f"(prefer={prefer}); refusing B200"
    )


def cmd_create(args: argparse.Namespace) -> None:
    existing = load_state()
    if existing and existing.get("id"):
        sys.exit(
            f"refusing create: {STATE_PATH} already has id={existing['id']}. "
            "Delete first or remove the state file if the instance is gone."
        )
    api_key = load_api_key()
    types_resp = api_request("GET", "/instances/types", api_key)
    if isinstance(types_resp, list):
        types = types_resp
    elif isinstance(types_resp, dict):
        types = (
            types_resp.get("instance_types")
            or types_resp.get("types")
            or types_resp.get("data")
            or []
        )
    else:
        sys.exit("unexpected /instances/types response shape")
    if not types:
        sys.exit("no instance types returned")

    pick = pick_instance_type(types, args.gpu)
    body = {
        "cloud": pick["cloud"],
        "region": pick["region"],
        "shade_instance_type": pick["shade_instance_type"],
        "shade_cloud": True,
        "name": INSTANCE_NAME,
        "os": pick["os"],
    }
    # Attach the project SSH key so the instance accepts our ed25519 key.
    ssh_key_id = ensure_ssh_key(api_key)
    body["ssh_key_id"] = ssh_key_id
    price_usd = pick.get("hourly_price_usd")
    print(
        f"creating {pick['gpu_type']} {pick['shade_instance_type']} "
        f"in {pick['cloud']}/{pick['region']} os={pick['os']} "
        f"hourly_price_cents={pick['hourly_price']} "
        f"hourly_price_usd={price_usd} ssh_key_id={ssh_key_id}",
        flush=True,
    )
    created = api_request("POST", "/instances/create", api_key, body)
    if not isinstance(created, dict):
        sys.exit("unexpected create response")
    instance_id = created.get("id") or (created.get("instance") or {}).get("id")
    if not instance_id:
        sys.exit(f"create response missing id: {json.dumps(created)[:500]}")

    state = {
        "id": instance_id,
        "ip": None,
        "ssh_user": None,
        "ssh_port": None,
        "shade_instance_type": pick["shade_instance_type"],
        "gpu_type": pick["gpu_type"],
        "cloud": pick["cloud"],
        "region": pick["region"],
        "os": pick["os"],
        "hourly_price": pick["hourly_price"],
        "hourly_price_usd": pick.get("hourly_price_usd"),
        "created_at": datetime.now(timezone.utc).isoformat(),
        "status": "pending",
    }
    save_state(state)
    print(f"created id={instance_id}; polling until active...", flush=True)

    deadline = time.time() + args.timeout
    while time.time() < deadline:
        info = api_request("GET", f"/instances/{instance_id}/info", api_key)
        if not isinstance(info, dict):
            time.sleep(10)
            continue
        # Some APIs nest under "instance"
        if "instance" in info and isinstance(info["instance"], dict):
            info = info["instance"]
        status = info.get("status") or info.get("state") or ""
        ip = info.get("ip") or info.get("public_ip") or info.get("ssh_ipv4")
        ssh_user = info.get("ssh_user") or info.get("username") or "shadeform"
        ssh_port = info.get("ssh_port") or 22
        hourly = info.get("hourly_price") or info.get("price") or state.get("hourly_price")
        state.update(
            {
                "status": status,
                "ip": ip,
                "ssh_user": ssh_user,
                "ssh_port": int(ssh_port) if ssh_port else 22,
                "hourly_price": hourly,
            }
        )
        save_state(state)
        print(f"  status={status} ip={ip}", flush=True)
        if str(status).lower() == "active" and ip:
            print(f"instance active: {json.dumps({k: state[k] for k in state if k != 'raw'})}")
            return
        time.sleep(15)
    sys.exit(f"timed out waiting for instance {instance_id} to become active")


def cmd_status(_args: argparse.Namespace) -> None:
    state = load_state()
    if not state or not state.get("id"):
        print("no local state (.shadeform-state.json missing)")
        return
    api_key = load_api_key()
    info = api_request("GET", f"/instances/{state['id']}/info", api_key)
    if isinstance(info, dict) and "instance" in info and isinstance(info["instance"], dict):
        info = info["instance"]
    if isinstance(info, dict):
        for k in ("status", "ip", "ssh_user", "ssh_port", "hourly_price"):
            if info.get(k) is not None:
                state[k] = info[k]
            # alternate keys
        if info.get("public_ip"):
            state["ip"] = info["public_ip"]
        if info.get("status") or info.get("state"):
            state["status"] = info.get("status") or info.get("state")
        save_state(state)
    print(json.dumps(state, indent=2))


def ssh_base(state: dict[str, Any]) -> list[str]:
    if not state.get("ip"):
        sys.exit("state has no ip; run status or wait for create")
    if not SSH_KEY_PATH.is_file():
        sys.exit(f"missing {SSH_KEY_PATH}")
    user = state.get("ssh_user") or "shadeform"
    port = str(state.get("ssh_port") or 22)
    return [
        "ssh",
        "-i",
        str(SSH_KEY_PATH),
        "-p",
        port,
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "UserKnownHostsFile=" + str(REPO_ROOT / ".shadeform_known_hosts"),
        f"{user}@{state['ip']}",
    ]


def cmd_ssh_cmd(args: argparse.Namespace) -> None:
    state = load_state()
    if not state:
        sys.exit("no .shadeform-state.json")
    cmd = args.command
    if not cmd:
        sys.exit("ssh-cmd requires a command string")
    full = ssh_base(state) + [cmd]
    # Do not print the key path contents; argv contains only the path.
    rc = subprocess.call(full)
    sys.exit(rc)


def cmd_delete(args: argparse.Namespace) -> None:
    if not args.yes_i_am_sure:
        sys.exit("refusing delete without --yes-i-am-sure")
    state = load_state()
    if not state or not state.get("id"):
        sys.exit("no instance id in state")
    api_key = load_api_key()
    instance_id = state["id"]
    print(f"deleting instance {instance_id}...", flush=True)
    api_request("POST", f"/instances/{instance_id}/delete", api_key, {})
    clear_state()
    print("deleted; cleared .shadeform-state.json")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("create", help="create one H100 (fallback H200)")
    c.add_argument("--gpu", default="H100", help="preferred gpu_type (default H100)")
    c.add_argument(
        "--timeout",
        type=int,
        default=1800,
        help="seconds to wait for active status",
    )
    c.set_defaults(func=cmd_create)

    s = sub.add_parser("status", help="show instance status")
    s.set_defaults(func=cmd_status)

    sc = sub.add_parser("ssh-cmd", help="run remote command via SSH")
    sc.add_argument("command", help="remote shell command")
    sc.set_defaults(func=cmd_ssh_cmd)

    d = sub.add_parser("delete", help="delete the instance")
    d.add_argument(
        "--yes-i-am-sure",
        action="store_true",
        help="required confirmation flag",
    )
    d.set_defaults(func=cmd_delete)
    return p


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
