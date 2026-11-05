#!/usr/bin/env python3
"""Run a benchmark with temporary CPU frequency limits, then restore them.

Run as your normal user, after `sudo -v` in the same terminal:
  python3 fixed_frequency.py --cpus 8 10 12 14 --khz 3000000 -- COMMAND ...

Only the frequency/cgroup helper runs as root. The command keeps your user and
environment. A private cpuset cgroup prevents benchmark workers from widening
their CPU masks. SKRIFT_EMBED_CPU_CGROUP_PARENT identifies its delegated parent.
SMT siblings are included in the frequency limits because they share cores.
Equal minimum/maximum limits request a fixed frequency; hardware power and
thermal constraints can still reduce it. --inspect only reads the plan.
"""

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import uuid


def cpu_ids(value):
    result = set()
    for item in value.replace(" ", ",").split(","):
        if not item:
            continue
        low, separator, high = item.partition("-")
        result.update(range(int(low), int(high) + 1) if separator else [int(low)])
    return result


def plan(cpus, khz):
    root = Path("/sys/devices/system/cpu")
    siblings = set()
    for cpu in cpus:
        if cpu < 0:
            raise ValueError("CPU IDs must be non-negative")
        siblings.update(cpu_ids((root / f"cpu{cpu}/topology/thread_siblings_list").read_text()))
    policies = []
    for policy in sorted({(root / f"cpu{cpu}/cpufreq").resolve(strict=True) for cpu in siblings}):
        minimum = int((policy / "cpuinfo_min_freq").read_text())
        maximum = int((policy / "cpuinfo_max_freq").read_text())
        if not minimum <= khz <= maximum:
            raise ValueError(f"{khz} kHz is outside {policy.name}'s {minimum}..{maximum} kHz range")
        policies.append({"path": str(policy),
                         "related_cpus": sorted(cpu_ids((policy / "related_cpus").read_text())),
                         "original_min_khz": int((policy / "scaling_min_freq").read_text()),
                         "original_max_khz": int((policy / "scaling_max_freq").read_text()),
                         "driver": (policy / "scaling_driver").read_text().strip()})
    return {"requested_cpus": cpus, "cpus_including_smt_siblings": sorted(siblings),
            "requested_khz": khz, "policies": policies}


def write_bounds(policy, minimum, maximum):
    path = Path(policy["path"])
    # Maintain min <= max throughout both setting and restoration.
    if maximum < int((path / "scaling_min_freq").read_text()):
        (path / "scaling_min_freq").write_text(f"{minimum}\n")
        (path / "scaling_max_freq").write_text(f"{maximum}\n")
    else:
        (path / "scaling_max_freq").write_text(f"{maximum}\n")
        (path / "scaling_min_freq").write_text(f"{minimum}\n")
    # CPUFreq's QoS notifier queues policy updates on a kernel workqueue.
    # A successful sysfs write does not mean the effective limits already
    # changed. Wait for both limits when applying and when restoring them.
    deadline = time.monotonic() + 5
    while True:
        actual = (int((path / "scaling_min_freq").read_text()),
                  int((path / "scaling_max_freq").read_text()))
        if actual == (minimum, maximum):
            return
        if time.monotonic() >= deadline:
            raise RuntimeError(f"{path.name}: limits did not settle after 5s: requested {(minimum, maximum)}, read back {actual}")
        time.sleep(0.01)


def emit(value):
    print(json.dumps(value), flush=True)


class CpuSet:
    """Constrain hosts while leaving the unprivileged controller unrestricted."""

    def __init__(self, cpus, controller_pid):
        self.root = Path("/sys/fs/cgroup")
        self.path = self.root / f"skrift-embed-bench-session-{uuid.uuid4().hex}"
        self.runner = self.path / "runner"
        self.benchmarks = self.path / "benchmarks"
        self.cpus = set(cpus)
        self.pid = controller_pid
        self.uid, self.gid = int(os.environ["SUDO_UID"]), int(os.environ["SUDO_GID"])
        process = Path("/proc") / str(self.pid)
        if self.pid <= 1 or process.stat().st_uid != self.uid:
            raise ValueError("controller PID must belong to the invoking user")
        self.start = self.process_start()
        membership = (process / "cgroup").read_text().splitlines()
        relative = next(line.removeprefix("0::") for line in membership if line.startswith("0::"))
        self.original = self.root / relative.lstrip("/")
        self.created = []
        self.enabled_root_cpuset = False
        self.moved = False

    def process_start(self):
        # starttime (field 22) guards against PID reuse during cleanup.
        return (Path("/proc") / str(self.pid) / "stat").read_text().rsplit(")", 1)[1].split()[19]

    def apply(self):
        if "cpuset" not in (self.root / "cgroup.controllers").read_text().split():
            raise RuntimeError("the unified cgroup hierarchy has no cpuset controller")
        if "cpuset" not in (self.root / "cgroup.subtree_control").read_text().split():
            self.enabled_root_cpuset = True
            (self.root / "cgroup.subtree_control").write_text("+cpuset\n")
        self.path.mkdir()
        self.created.append(self.path)
        (self.path / "cgroup.subtree_control").write_text("+cpuset +cpu +memory +pids\n")
        # Migration between runner and benchmarks requires write access to
        # cgroup.procs in their common ancestor, even with delegated children.
        os.chown(self.path / "cgroup.procs", self.uid, self.gid)
        for child in (self.runner, self.benchmarks):
            child.mkdir()
            self.created.append(child)
        (self.benchmarks / "cpuset.mems").write_text((self.path / "cpuset.mems.effective").read_text())
        (self.benchmarks / "cpuset.cpus").write_text(",".join(map(str, sorted(self.cpus))) + "\n")
        effective = cpu_ids((self.benchmarks / "cpuset.cpus.effective").read_text())
        if effective != self.cpus:
            raise RuntimeError(f"cpuset requested {sorted(self.cpus)}, obtained {sorted(effective)}")
        (self.benchmarks / "cgroup.subtree_control").write_text("+cpuset +cpu +memory +pids\n")
        # Keep this group's CPU ceiling root-owned; the user creates host
        # subgroups underneath it, which cannot exceed their parent's ceiling.
        for entry in (self.benchmarks, *(self.benchmarks / name for name in
                      ("cgroup.procs", "cgroup.threads", "cgroup.subtree_control"))):
            os.chown(entry, self.uid, self.gid)
        self.moved = True
        (self.runner / "cgroup.procs").write_text(f"{self.pid}\n")
        return {"cpu_cgroup_parent": str(self.benchmarks),
                "kernel_cpuset_cpus": sorted(effective)}

    def restore(self):
        errors, retained = [], []
        # Never kill the wrapper: move it out before cleaning its descendants.
        if self.moved:
            try:
                if self.process_start() == self.start:
                    (self.original / "cgroup.procs").write_text(f"{self.pid}\n")
                self.moved = False
            except FileNotFoundError:
                if not (Path("/proc") / str(self.pid)).exists():
                    self.moved = False
                else:
                    errors.append("original controller cgroup disappeared; leaving runner intact")
            except OSError as error:
                errors.append(str(error))
        for child in (self.benchmarks, self.runner):
            if child not in self.created or (child == self.runner and self.moved):
                continue
            try:
                (child / "cgroup.kill").write_text("1\n")
                deadline = time.monotonic() + 5
                while "populated 1" in (child / "cgroup.events").read_text():
                    if time.monotonic() >= deadline:
                        raise RuntimeError(f"{child}: processes did not exit during cleanup")
                    time.sleep(.02)
                for directory, _, _ in os.walk(child, topdown=False):
                    Path(directory).rmdir()
                self.created.remove(child)
            except (OSError, RuntimeError) as error:
                errors.append(str(error))
        if self.path in self.created:
            try:
                self.path.rmdir()
            except OSError as error:
                errors.append(str(error))
        if self.enabled_root_cpuset:
            # Avoid undoing a concurrent user's cpuset configuration.
            in_use = bool(errors)
            for child in self.root.iterdir():
                if not child.is_dir():
                    continue
                try:
                    in_use |= ("cpuset" in (child / "cgroup.subtree_control").read_text().split()
                               or bool((child / "cpuset.cpus").read_text().strip())
                               or bool((child / "cpuset.mems").read_text().strip()))
                except FileNotFoundError:
                    continue
            if in_use:
                retained.append("root cpuset controller remains enabled because another group uses it or cleanup failed")
            else:
                try:
                    (self.root / "cgroup.subtree_control").write_text("-cpuset\n")
                except OSError as error:
                    errors.append(str(error))
        return errors, retained


def helper(configuration, controller_pid):
    if os.geteuid() != 0:
        raise PermissionError("frequency changes require root")
    def interrupted(signum, frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    modified = []
    cpuset = CpuSet(configuration["requested_cpus"], controller_pid)
    try:
        confinement = cpuset.apply()
        for policy in configuration["policies"]:
            modified.append(policy)
            write_bounds(policy, configuration["requested_khz"], configuration["requested_khz"])
        emit({"state": "frequency_limits_applied", **configuration, **confinement})
        # EOF also restores limits if the unprivileged wrapper exits.
        for command in sys.stdin:
            if command.strip() != "restore":
                raise ValueError("unexpected frequency helper command")
            break
    finally:
        operation_error = sys.exc_info()[1]
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        errors = []
        for policy in reversed(modified):
            try:
                write_bounds(policy, policy["original_min_khz"], policy["original_max_khz"])
            except OSError as error:
                errors.append(str(error))
            except RuntimeError as error:
                errors.append(str(error))
        try:
            cleanup_errors, retained = cpuset.restore()
            errors.extend(cleanup_errors)
        except OSError as error:
            errors.append(str(error))
            retained = []
        emit({"state": "frequency_limits_restored" if not errors else "restore_failed", "errors": errors,
              "cpuset_cleanup_notes": retained,
              "operation_error": None if operation_error is None else str(operation_error) or type(operation_error).__name__})
        if errors and operation_error is None:
            raise RuntimeError("; ".join(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cpus", type=int, nargs="+", required=True)
    parser.add_argument("--khz", type=int, required=True)
    parser.add_argument("--inspect", action="store_true")
    parser.add_argument("--helper", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--controller-pid", type=int, help=argparse.SUPPRESS)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.khz <= 0 or len(args.cpus) != len(set(args.cpus)):
        parser.error("frequency must be positive and CPU IDs distinct")
    configuration = plan(sorted(args.cpus), args.khz)
    if args.inspect:
        emit({**configuration, "kernel_cpuset_cpus": sorted(args.cpus),
              "root_cpuset_already_enabled": "cpuset" in Path("/sys/fs/cgroup/cgroup.subtree_control").read_text().split()})
        return 0
    if args.helper:
        if args.controller_pid is None:
            parser.error("helper requires a controller PID")
        helper(configuration, args.controller_pid)
        return 0
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("specify the benchmark command after --, or use --inspect")
    if os.geteuid() == 0:
        parser.error("run the wrapper as your normal user; only its frequency helper needs sudo")
    privileged = subprocess.Popen(
        ["sudo", "-n", sys.executable, str(Path(__file__).resolve()), "--helper",
         "--controller-pid", str(os.getpid()),
         "--cpus", *map(str, args.cpus), "--khz", str(args.khz)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        ready = privileged.stdout.readline()
        if not ready:
            raise PermissionError("CPU frequency access unavailable. Run sudo -v in the same terminal, then rerun this command.")
        state = json.loads(ready)
        if state["state"] != "frequency_limits_applied":
            raise RuntimeError(state)
        emit(state)
        environment = os.environ.copy()
        environment["SKRIFT_EMBED_FIXED_CPU_KHZ"] = str(args.khz)
        environment["SKRIFT_EMBED_CPU_CGROUP_PARENT"] = state["cpu_cgroup_parent"]
        environment["SKRIFT_EMBED_CPUSET_CPUS"] = ",".join(map(str, state["kernel_cpuset_cpus"]))
        return subprocess.run(command, env=environment).returncode
    finally:
        privileged.stdin.close()
        remaining = privileged.stdout.read()
        if remaining:
            print(remaining, end="", flush=True)
        if privileged.wait(timeout=60) != 0 and sys.exc_info()[0] is None:
            raise RuntimeError("frequency helper failed; inspect its output above")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
