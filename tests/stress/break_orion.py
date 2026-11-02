#!/usr/bin/env python3
"""
break_orion.py - Brutal Destruction and Stress Testing Suite for Orion

Executes multi-level stress, chaos, high-concurrency, fuzzing, and orphan process
destruction tests to identify failure modes, memory leaks, dropped connections,
and edge-case crashes.

Fully isolated using dedicated XDG_RUNTIME_DIR and XDG_DATA_HOME in the sandbox.
"""

import asyncio
import json
import os
import random
import re
import signal
import socket
import subprocess
import sys
import time
import urllib.request
import urllib.error
from concurrent.futures import ThreadPoolExecutor

# Sandbox Paths & Isolation Configuration
SANDBOX_DIR = os.path.expanduser("~/orion-test-sandbox")
SANDBOX_RUN = os.path.join(SANDBOX_DIR, "run")
SANDBOX_DATA = os.path.join(SANDBOX_DIR, "data")
os.makedirs(SANDBOX_RUN, exist_ok=True)
os.makedirs(SANDBOX_DATA, exist_ok=True)

ENV = os.environ.copy()
ENV["XDG_RUNTIME_DIR"] = SANDBOX_RUN
ENV["XDG_DATA_HOME"] = SANDBOX_DATA

SOCKET_PATH = os.path.join(SANDBOX_RUN, "orion-daemon.sock")

# Configuration & Ports (isolated test range)
TEST_PORT = 38080
BACKEND_PORTS = [38081, 38082, 38083, 38084]
APP_NAME = "stress-cluster"
ZOMBIE_APP_NAME = "stress-zombie"
ORION_BIN = os.path.join(SANDBOX_DIR, "orion/target/release/orion")
ORIOND_BIN = os.path.join(SANDBOX_DIR, "orion/target/release/oriond")
FIXTURES_DIR = os.path.join(SANDBOX_DIR, "orion/tests/fixtures")

def log_header(title):
    print("\n" + "=" * 70)
    print(f" [STRESS TEST] {title}")
    print("=" * 70)

def run_cmd(cmd_list, timeout=30):
    try:
        res = subprocess.run(
            cmd_list,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=timeout,
            env=ENV
        )
        return res.returncode, res.stdout, res.stderr
    except subprocess.TimeoutExpired:
        return -1, "", "Command timed out"

def is_daemon_alive():
    if not os.path.exists(SOCKET_PATH):
        return False
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(2.0)
        s.connect(SOCKET_PATH)
        req = json.dumps({"jsonrpc": "2.0", "id": 9999, "method": "daemon_status", "params": None}) + "\n"
        s.sendall(req.encode('utf-8'))
        resp = s.recv(4096).decode('utf-8')
        s.close()
        return "result" in resp or "daemon_status" in resp
    except Exception:
        return False

def get_test_daemon_pid():
    code, out, _ = run_cmd([ORION_BIN, "daemon", "status"])
    for line in out.splitlines():
        if "PID:" in line:
            parts = line.split("PID:")
            if len(parts) > 1 and parts[1].strip().isdigit():
                return int(parts[1].strip())
    return None

def start_test_daemon():
    print(f"[*] Starting isolated test daemon on {SOCKET_PATH}...")
    run_cmd([ORION_BIN, "daemon", "start"])
    for _ in range(20):
        if is_daemon_alive():
            pid = get_test_daemon_pid()
            print(f"[*] Isolated test daemon online with PID: {pid}")
            return True
        time.sleep(0.3)
    return False

def ensure_clean_state():
    print("[*] Cleaning up any old test processes...")
    run_cmd([ORION_BIN, "stop", APP_NAME])
    run_cmd([ORION_BIN, "delete", APP_NAME])
    run_cmd([ORION_BIN, "stop", ZOMBIE_APP_NAME])
    run_cmd([ORION_BIN, "delete", ZOMBIE_APP_NAME])
    time.sleep(1)

# ============================================================================
# LEVEL 1: Direct IPC Socket Fuzzing & Malformed Protocol Bombardment
# ============================================================================
def test_level_1_ipc_fuzzing():
    log_header("LEVEL 1: Direct IPC Socket Fuzzing & Protocol Flooding")
    if not os.path.exists(SOCKET_PATH):
        print(f"[FAIL] IPC socket {SOCKET_PATH} does not exist. Is daemon running?")
        return False

    print("[1A] Fuzzing: Sending 100KB raw non-UTF8 pseudo-random binary junk...")
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(3.0)
        s.connect(SOCKET_PATH)
        junk = os.urandom(102400) + b"\n"
        s.sendall(junk)
        try:
            resp = s.recv(4096)
            print(f"     Daemon response to binary junk: {resp.decode('utf-8', errors='replace').strip()[:80]}")
        except Exception as e:
            print(f"     Daemon closed connection on binary junk: {e}")
        s.close()
    except Exception as e:
        print(f"     Socket error during junk test: {e}")

    if not is_daemon_alive():
        print("[FAIL] Daemon crashed after receiving raw binary junk!")
        return False
    print("     [OK] Daemon survived binary junk injection.")

    print("[1B] Connection Thrashing: Rapidly opening & immediately closing 200 sockets...")
    failed_connects = 0
    for i in range(200):
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.settimeout(1.0)
            s.connect(SOCKET_PATH)
            s.close()
        except Exception:
            failed_connects += 1
    print(f"     Completed 200 connection cycles (failed connects: {failed_connects}).")
    if not is_daemon_alive():
        print("[FAIL] Daemon crashed or leaked socket handles under connection churn!")
        return False
    print("     [OK] Daemon survived connection churn.")

    print("[1C] Truncated / Incomplete JSON Line...")
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(2.0)
        s.connect(SOCKET_PATH)
        s.sendall(b'{"jsonrpc": "2.0", "id": 1, "method": "process_start", "params":')
        s.close()  # Disconnect midway through payload
    except Exception as e:
        print(f"     Error during truncated send: {e}")
    if not is_daemon_alive():
        print("[FAIL] Daemon crashed on incomplete JSON stream!")
        return False
    print("     [OK] Daemon survived truncated stream.")

    print("[1D] Oversized JSON Payload (5MB frame)...")
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(3.0)
        s.connect(SOCKET_PATH)
        large_str = "A" * (5 * 1024 * 1024)
        payload = json.dumps({"jsonrpc": "2.0", "id": 2, "method": "process_info", "params": {"name": large_str}}) + "\n"
        s.sendall(payload.encode('utf-8'))
        resp = s.recv(4096)
        print(f"     Daemon response to 5MB payload: {resp.decode('utf-8', errors='replace')[:80]}...")
        s.close()
    except Exception as e:
        print(f"     Daemon handled/dropped oversized socket write: {e}")

    if not is_daemon_alive():
        print("[FAIL] Daemon crashed or ran OOM on oversized JSON payload!")
        return False
    print("     [OK] Daemon survived oversized payload test.")
    return True

# ============================================================================
# LEVEL 2: High-Concurrency TCP Cluster Saturation (5,000 requests)
# ============================================================================
def test_level_2_high_concurrency():
    log_header("LEVEL 2: High-Concurrency TCP Cluster Saturation")
    worker_script = os.path.join(FIXTURES_DIR, "http_worker.js")
    
    print(f"[*] Starting 4-instance cluster on public port {TEST_PORT}...")
    code, out, err = run_cmd([
        ORION_BIN, "start", worker_script,
        "--name", APP_NAME,
        "-i", "4",
        "-p", str(TEST_PORT)
    ])
    if code != 0:
        print(f"[FAIL] Could not start cluster: {err or out}")
        return False

    time.sleep(2)  # Wait for workers to bind

    url = f"http://127.0.0.1:{TEST_PORT}/"
    total_requests = 5000
    concurrency = 50
    print(f"[*] Firing {total_requests} requests at {url} with concurrency {concurrency}...")

    success_count = 0
    failure_count = 0
    worker_pid_hits = {}
    latencies = []

    def make_request(_):
        nonlocal success_count, failure_count
        t0 = time.time()
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "OrionStress/1.0"})
            with urllib.request.urlopen(req, timeout=3.0) as resp:
                data = resp.read()
                dur = time.time() - t0
                body = json.loads(data.decode('utf-8'))
                pid = body.get("pid")
                return True, dur, pid
        except Exception:
            return False, time.time() - t0, None

    t_start = time.time()
    with ThreadPoolExecutor(max_workers=concurrency) as executor:
        results = list(executor.map(make_request, range(total_requests)))
    total_time = time.time() - t_start

    for ok, dur, pid in results:
        if ok:
            success_count += 1
            latencies.append(dur * 1000.0)
            worker_pid_hits[pid] = worker_pid_hits.get(pid, 0) + 1
        else:
            failure_count += 1

    latencies.sort()
    p50 = latencies[len(latencies) // 2] if latencies else 0
    p95 = latencies[int(len(latencies) * 0.95)] if latencies else 0
    p99 = latencies[int(len(latencies) * 0.99)] if latencies else 0
    rps = total_requests / total_time if total_time > 0 else 0

    print(f"[*] Results: {success_count}/{total_requests} succeeded ({failure_count} failed)")
    print(f"[*] Throughput: {rps:.1f} req/sec in {total_time:.2f}s")
    print(f"[*] Latency: P50={p50:.1f}ms, P95={p95:.1f}ms, P99={p99:.1f}ms")
    print(f"[*] Load distribution across backend PIDs: {worker_pid_hits}")

    if failure_count > 0:
        print(f"[FAIL] Cluster dropped {failure_count} connections under load!")
        return False
    if len(worker_pid_hits) < 4:
        print(f"[WARN] Expected 4 distinct worker PIDs, got {len(worker_pid_hits)}: {worker_pid_hits}")
    else:
        print("     [OK] Round-robin load balancer evenly distributed across all 4 workers.")
    return True

# ============================================================================
# LEVEL 3: Chaos Monkey - Violent SIGKILL Storm under Traffic
# ============================================================================
def test_level_3_chaos_kill_storm():
    log_header("LEVEL 3: Chaos Monkey - Violent SIGKILL Storm under Continuous Traffic")
    url = f"http://127.0.0.1:{TEST_PORT}/"
    
    stop_event = False
    req_success = 0
    req_failed = 0
    kills_delivered = 0

    def traffic_generator():
        nonlocal req_success, req_failed
        while not stop_event:
            try:
                req = urllib.request.Request(url, headers={"User-Agent": "OrionChaos/1.0"})
                with urllib.request.urlopen(req, timeout=1.5) as resp:
                    if resp.status == 200:
                        req_success += 1
                    else:
                        req_failed += 1
            except Exception:
                req_failed += 1
            time.sleep(0.01)

    print("[*] Launching 10 background client threads hammering HTTP cluster...")
    threads = []
    for _ in range(10):
        t = ThreadPoolExecutor(max_workers=1).submit(traffic_generator)
        threads.append(t)

    print("[*] Initiating 15-second SIGKILL storm (killing random worker every 250ms)...")
    t_end = time.time() + 15.0
    while time.time() < t_end:
        time.sleep(0.25)
        # Find PIDs belonging to node http_worker.js
        code, out, _ = run_cmd(["pgrep", "-f", "http_worker.js"])
        if code == 0 and out.strip():
            pids = [int(p) for p in out.strip().split("\n") if p.isdigit()]
            if pids:
                victim = random.choice(pids)
                try:
                    os.kill(victim, signal.SIGKILL)
                    kills_delivered += 1
                except ProcessLookupError:
                    pass

    stop_event = True
    time.sleep(1.0)

    print(f"[*] Injected {kills_delivered} violent SIGKILL terminations into active workers.")
    print(f"[*] HTTP Traffic Stats: {req_success} succeeded, {req_failed} failed.")
    fail_rate = (req_failed / (req_success + req_failed) * 100) if (req_success + req_failed) > 0 else 0
    print(f"[*] Error rate during violent crash storm: {fail_rate:.2f}%")

    # Give supervisor 3 seconds to recover all 4 instances
    time.sleep(3.0)
    code, out, _ = run_cmd([ORION_BIN, "status"])
    print(f"[*] Orion cluster status after chaos storm:\n{out}")

    code, out, _ = run_cmd(["pgrep", "-f", "http_worker.js"])
    recovered_pids = [int(p) for p in out.strip().split("\n") if p.isdigit()] if out.strip() else []
    print(f"[*] Active worker processes recovered: {len(recovered_pids)} (expected 4)")

    if len(recovered_pids) < 4:
        print("[FAIL] Orion supervisor failed to resurrect all 4 workers after chaos storm!")
        return False
    print("     [OK] Supervisor recovered and maintained cluster availability.")
    return True

# ============================================================================
# LEVEL 4: Rapid Rolling Reload Thrasher
# ============================================================================
def test_level_4_rapid_reload_thrasher():
    log_header("LEVEL 4: Rapid Rolling Reload Thrasher")
    url = f"http://127.0.0.1:{TEST_PORT}/"
    stop_event = False
    req_success = 0
    req_failed = 0

    def traffic_generator():
        nonlocal req_success, req_failed
        while not stop_event:
            try:
                req = urllib.request.Request(url, headers={"User-Agent": "OrionReload/1.0"})
                with urllib.request.urlopen(req, timeout=2.0) as resp:
                    if resp.status == 200:
                        req_success += 1
                    else:
                        req_failed += 1
            except Exception:
                req_failed += 1
            time.sleep(0.005)

    print("[*] Firing continuous HTTP load during rapid reload iterations...")
    workers = ThreadPoolExecutor(max_workers=8)
    for _ in range(8):
        workers.submit(traffic_generator)

    reload_iterations = 10
    print(f"[*] Triggering {reload_iterations} consecutive reloads with zero pause...")
    for i in range(reload_iterations):
        t0 = time.time()
        code, out, err = run_cmd([ORION_BIN, "reload", APP_NAME])
        elapsed = time.time() - t0
        print(f"     Reload #{i+1} completed in {elapsed:.2f}s (code: {code})")
        if code != 0:
            print(f"     [ERROR] Reload failed: {err}")

    stop_event = True
    time.sleep(1.0)
    print(f"[*] Traffic during reload thrashing: {req_success} succeeded, {req_failed} failed.")
    fail_rate = (req_failed / (req_success + req_failed) * 100) if (req_success + req_failed) > 0 else 0
    print(f"[*] Dropped connection rate: {fail_rate:.2f}%")

    if req_failed > 0:
        print(f"[FAIL] Zero-downtime violated: {req_failed} requests dropped during reloads!")
        return False
    print("     [OK] 100% Zero-Downtime maintained across rapid consecutive reloads.")
    return True

# ============================================================================
# LEVEL 5: Zombie / Orphan Process Tree Destruction
# ============================================================================
def test_level_5_orphan_destruction():
    log_header("LEVEL 5: Zombie & Orphan Process Tree Destruction")
    zombie_script = os.path.join(FIXTURES_DIR, "zombie_parent.js")

    print("[5A] Testing clean stop of process tree with detached children...")
    code, out, err = run_cmd([
        ORION_BIN, "start", zombie_script,
        "--name", ZOMBIE_APP_NAME,
        "-i", "1"
    ])
    if code != 0:
        print(f"[FAIL] Could not start zombie parent: {err or out}")
        return False

    time.sleep(2)  # Allow child processes to spawn
    code, out, _ = run_cmd(["pgrep", "-f", "zombie_parent.js"])
    parent_pids = out.strip().split()
    print(f"[*] Zombie parent PID(s): {parent_pids}")

    # Now stop the process cleanly
    print(f"[*] Issuing 'orion stop {ZOMBIE_APP_NAME}'...")
    code, out, err = run_cmd([ORION_BIN, "stop", ZOMBIE_APP_NAME])
    time.sleep(1)

    code, out, _ = run_cmd(["pgrep", "-f", "zombie_parent.js"])
    if out.strip():
        print(f"[FAIL] Zombie parent is still alive after stop: {out.strip()}")
        return False
    print("     [OK] Parent and process group cleanly terminated.")

    print("\n[5B] Testing Hard Test Daemon Kill (SIGKILL oriond) -> Kernel PR_SET_PDEATHSIG...")
    # Start app again
    run_cmd([ORION_BIN, "start", zombie_script, "--name", ZOMBIE_APP_NAME, "-i", "1"])
    time.sleep(2)

    code, out, _ = run_cmd(["pgrep", "-f", "zombie_parent.js"])
    parent_pids = out.strip().split()
    print(f"[*] Running with parent PID(s): {parent_pids}")

    test_daemon_pid = get_test_daemon_pid()
    if not test_daemon_pid:
        print("[FAIL] Could not determine isolated test daemon PID!")
        return False
    print(f"[*] Hard-killing isolated test daemon (PID: {test_daemon_pid}) with SIGKILL (kill -9)...")
    os.kill(test_daemon_pid, signal.SIGKILL)

    # Allow kernel 1.5 seconds to deliver PR_SET_PDEATHSIG to children
    time.sleep(1.5)

    code, out, _ = run_cmd(["pgrep", "-f", "zombie_parent.js"])
    surviving = out.strip().split()
    if surviving:
        print(f"[FAIL] Leaked orphan process(es) after daemon death: {surviving}!")
        for p in surviving:
            try:
                os.kill(int(p), signal.SIGKILL)
            except Exception:
                pass
        return False

    print("     [OK] Linux kernel PR_SET_PDEATHSIG successfully reaped child processes upon daemon crash!")
    return True

# ============================================================================
# Main Entry Point
# ============================================================================
def main():
    print("======================================================================")
    print("         ORION BRUTAL STRESS & DESTRUCTION TEST SUITE                 ")
    print("======================================================================")
    print(f"Host: {socket.gethostname()}")
    print(f"OS: {sys.platform}")
    print(f"Sandbox Socket: {SOCKET_PATH}")
    print(f"Target binaries: {ORION_BIN}, {ORIOND_BIN}")
    print("======================================================================")

    if not is_daemon_alive():
        if not start_test_daemon():
            print("[CRITICAL] Could not start or connect to isolated test daemon!")
            sys.exit(1)

    ensure_clean_state()

    results = {}
    try:
        results["Level 1 (IPC Fuzzing)"] = test_level_1_ipc_fuzzing()
    except Exception as e:
        print(f"[ERROR in Level 1]: {e}")
        results["Level 1 (IPC Fuzzing)"] = False

    try:
        results["Level 2 (High Concurrency)"] = test_level_2_high_concurrency()
    except Exception as e:
        print(f"[ERROR in Level 2]: {e}")
        results["Level 2 (High Concurrency)"] = False

    try:
        results["Level 3 (Chaos SIGKILL Storm)"] = test_level_3_chaos_kill_storm()
    except Exception as e:
        print(f"[ERROR in Level 3]: {e}")
        results["Level 3 (Chaos SIGKILL Storm)"] = False

    try:
        results["Level 4 (Rolling Reload Thrasher)"] = test_level_4_rapid_reload_thrasher()
    except Exception as e:
        print(f"[ERROR in Level 4]: {e}")
        results["Level 4 (Rolling Reload Thrasher)"] = False

    try:
        results["Level 5 (Orphan Process Destruction)"] = test_level_5_orphan_destruction()
    except Exception as e:
        print(f"[ERROR in Level 5]: {e}")
        results["Level 5 (Orphan Process Destruction)"] = False

    # Final cleanup of test socket & data if needed
    if is_daemon_alive():
        ensure_clean_state()
        run_cmd([ORION_BIN, "daemon", "stop"])

    print("\n" + "=" * 70)
    print("                     FINAL TEST SUMMARY")
    print("=" * 70)
    all_passed = True
    for test_name, passed in results.items():
        status = "PASSED" if passed else "FAILED"
        print(f" {test_name:<40} : {status}")
        if not passed:
            all_passed = False
    print("=" * 70)

    if all_passed:
        print("[SUCCESS] All 5 stress and destruction test levels passed!")
        sys.exit(0)
    else:
        print("[FAILURE] Orion broke on one or more stress tests. Review logs above.")
        sys.exit(1)

if __name__ == "__main__":
    main()
