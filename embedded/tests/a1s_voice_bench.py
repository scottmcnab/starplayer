#!/usr/bin/env python3
"""Capture or strictly replay ESP32-A1S VOICE_BENCH transcripts."""

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request
from typing import NamedTuple


SCHEMA_VERSION = 1
SAMPLE_RATE_HZ = 48_000
DESCRIPTOR_FRAMES = 256
DESCRIPTOR_DEADLINE_US = 5_333
PASS_RENDER_MAX_US = 4_266
HEAP_FLOOR_BYTES = 8 * 1024
UART_BAUD = 115_200
RESET_PULSE_SECONDS = 0.1
RUST_MIN_STACK_BYTES = 256 * 1024 * 1024
VOICE_PLATEAU_SETTLE_MS = 5_000
WEB_LOAD_SETTLE_MS = 5_000
WEB_LOAD_STALL_MS = 5_000
MODES = ("audio", "web")
FILTERS = (0, 1)
WEB_LOAD_KEYS = ("web_http_ok", "web_upload_ok", "web_ws_ok")
HOST_WEB_LOAD_KEYS = (
    "host_http_ok", "host_http_fail", "host_upload_ok", "host_upload_fail",
    "host_ws_ok", "host_ws_fail",
)

COMMON_KEYS = {"v", "seq", "kind"}
REQUIRED_KEYS = {
    "START": COMMON_KEYS | {
        "case", "mode", "filtered", "channels", "voices", "rate", "descriptor_frames",
        "duration_s", "phase", "axis", "low", "high", "dma_base",
    },
    "SAMPLE": COMMON_KEYS | {
        "case", "elapsed_ms", "frames", "active", "peak_active", "render_max_us",
        "render_p50_us", "render_p95_us", "misses", "underruns", "dma_errors",
        "warnings", "steals", "heap_internal", "heap_external", *WEB_LOAD_KEYS,
    },
    "END": COMMON_KEYS | {
        "case", "elapsed_ms", "frames", "active", "peak_active", "render_max_us",
        "render_p50_us", "render_p95_us", "misses", "underruns", "dma_errors",
        "warnings", "steals", "heap_internal", "heap_external", *WEB_LOAD_KEYS, "status", "reason",
    },
    "RESULT": COMMON_KEYS | {
        "mode", "filtered", "channel_limit", "voice_limit", "pass_case", "reject_case",
    },
}
INTEGER_KEYS = {
    "v", "seq", "filtered", "channels", "voices", "rate", "descriptor_frames",
    "duration_s", "low", "high", "dma_base", "elapsed_ms", "frames", "active",
    "peak_active", "render_max_us", "render_p50_us", "render_p95_us", "misses",
    "underruns", "dma_errors", "warnings", "steals", "heap_internal", "heap_external",
    "channel_limit", "voice_limit", *WEB_LOAD_KEYS, *HOST_WEB_LOAD_KEYS,
}
COUNTER_KEYS = ("elapsed_ms", "frames", "peak_active", "render_max_us", "misses", "underruns", "dma_errors", "warnings", "steals")


class ValidationError(ValueError):
    """A transcript cannot prove a valid benchmark."""


class CaseSpec(NamedTuple):
    name: str
    mode: str
    filtered: int
    channels: int
    voices: int
    duration_s: int
    phase: str
    axis: str
    low: int
    high: int


class SearchController:
    """Deterministic upper-midpoint bisection required by the I9 plan."""

    def __init__(self):
        self.stage = "voices"
        self.channels = 64
        self.low = 32
        self.high = 256
        self.current = 256
        self.best_pass = None
        self.best_reject = None
        self.channel_reject = None
        self.done = False

    def candidate(self):
        axis = "channels" if self.stage == "channels" else "voices"
        voices = 32 if self.stage == "channels" else self.current
        channels = self.current if self.stage == "channels" else self.channels
        return axis, self.low, self.high, channels, voices

    def observe(self, passed):
        if self.done:
            raise ValidationError("qualification continued after bisection converged")
        if passed:
            self.best_pass = self.current
            self.low = self.current
        else:
            self.best_reject = self.current
            self.high = self.current - 1

        if self.low > self.high or (self.low == self.high and self.best_pass == self.low):
            if self.stage == "voices" and self.channels == 64 and self.best_pass is None:
                self.stage = "channels"
                self.low, self.high, self.current = 1, 63, 32
                self.best_pass, self.best_reject = None, 64
                return
            if self.stage == "channels":
                if self.best_pass is None:
                    self.channels = 1
                    self.stage = "voices_at_one_channel"
                    self.low, self.high, self.current = 1, 31, 31
                    self.best_pass, self.best_reject = None, 32
                    return
                self.channels = self.best_pass
                self.channel_reject = self.best_reject
                self.stage = "voices_at_channel"
                self.low, self.high, self.current = 32, 256, 256
                self.best_pass, self.best_reject = 32, None
                return
            self.done = True
            return
        next_candidate = (self.low + self.high + 1) // 2
        if self.best_pass is None and self.low == self.high:
            next_candidate = self.low
        self.current = next_candidate

    def rejection_boundary(self):
        """Return the adjacent rejected axis/channels/voices retained by the search."""
        if not self.done:
            raise ValidationError("rejection boundary requested before bisection converged")
        if self.best_reject is not None:
            return "voices", self.channels, self.best_reject
        if self.channel_reject is not None:
            return "channels", self.channel_reject, 32
        return None

    def has_zero_capacity_boundary(self):
        """Whether the final native 1x1 qualification proved that no voice passes."""
        return (
            self.done
            and self.stage == "voices_at_one_channel"
            and self.channels == 1
            and self.best_pass is None
            and self.best_reject == 1
        )


def next_stable_voice_candidate(channels, lower_pass, upper_reject, qualification_outcomes, soaked_voices):
    """Choose the next bounded voice candidate after a provisional long-soak failure."""
    low = 1 if lower_pass is None else lower_pass + 1
    high = upper_reject - 1
    if low > high:
        return None
    known_passes = [
        voices for (candidate_channels, voices), passed in qualification_outcomes.items()
        if candidate_channels == channels and passed and low <= voices <= high and voices not in soaked_voices
    ]
    if known_passes:
        return max(known_passes), None
    voices = (low + high + 1) // 2
    bounds = None if (channels, voices) in qualification_outcomes else (low, high)
    return voices, bounds


def parse_record(line, line_number):
    """Parse one machine record, retaining unknown future keys."""
    marker = "VOICE_BENCH "
    position = line.find(marker)
    if position < 0:
        return None
    fields = {}
    for token in line[position + len(marker):].strip().split():
        if "=" not in token:
            raise ValidationError(f"line {line_number}: malformed token {token!r}")
        key, value = token.split("=", 1)
        if not key or not value or key in fields:
            raise ValidationError(f"line {line_number}: malformed or duplicate key {key!r}")
        fields[key] = value
    try:
        version = int(fields.get("v", ""))
    except ValueError as error:
        raise ValidationError(f"line {line_number}: invalid schema version") from error
    if version != SCHEMA_VERSION:
        raise ValidationError(f"line {line_number}: unsupported schema version {version}")
    kind = fields.get("kind")
    if kind not in REQUIRED_KEYS:
        raise ValidationError(f"line {line_number}: unknown record kind {kind!r}")
    missing = sorted(REQUIRED_KEYS[kind] - fields.keys())
    if missing:
        raise ValidationError(f"line {line_number}: missing keys {','.join(missing)}")
    for key in INTEGER_KEYS & fields.keys():
        try:
            fields[key] = int(fields[key])
        except ValueError as error:
            raise ValidationError(f"line {line_number}: {key} is not an integer") from error
        if fields[key] < 0:
            raise ValidationError(f"line {line_number}: {key} is negative")
    fields["line"] = line_number
    return fields


def case_passes(record, start):
    """Apply the benchmark's complete pass predicate to an END record."""
    elapsed_ms = record["elapsed_ms"]
    rate_error = abs(record["frames"] * 1000 - elapsed_ms * SAMPLE_RATE_HZ)
    rate_tolerance = max(DESCRIPTOR_FRAMES * 2000, elapsed_ms * SAMPLE_RATE_HZ // 100)
    return (
        record["active"] == start["voices"]
        and record["peak_active"] >= start["voices"]
        and record["render_max_us"] <= PASS_RENDER_MAX_US
        and record["misses"] == 0
        and record["underruns"] == 0
        and record["dma_errors"] == start["dma_base"]
        and record["warnings"] == 0
        and record["steals"] > 0
        and record["heap_internal"] >= HEAP_FLOOR_BYTES
        and rate_error <= rate_tolerance
        and (start["mode"] != "web" or all(record[key] > 0 for key in WEB_LOAD_KEYS))
    )


def transport_rate_is_valid(record):
    rate_error = abs(record["frames"] * 1000 - record["elapsed_ms"] * SAMPLE_RATE_HZ)
    rate_tolerance = max(DESCRIPTOR_FRAMES * 2000, record["elapsed_ms"] * SAMPLE_RATE_HZ // 100)
    return rate_error <= rate_tolerance


def web_load_progress_failed(record, previous, last_advance_ms):
    """Update per-class progress times and apply the positive/five-second stall gate."""
    regressed = False
    for index, key in enumerate(WEB_LOAD_KEYS):
        previous_count = 0 if previous is None else previous[key]
        if record[key] > previous_count:
            last_advance_ms[index] = record["elapsed_ms"]
        elif record[key] < previous_count:
            regressed = True
    if record["elapsed_ms"] < WEB_LOAD_SETTLE_MS:
        return False
    return (
        regressed
        or any(record[key] == 0 for key in WEB_LOAD_KEYS)
        or any(record["elapsed_ms"] - advanced_ms > WEB_LOAD_STALL_MS for advanced_ms in last_advance_ms)
    )


def validate_transcript(lines, short_seconds=60, long_seconds=600):
    """Validate a transcript and return its stable JSON representation."""
    records = []
    expected_sequence = None
    fatal_words = ("panic", "backtrace", "watchdog", "brownout", "rst:", "fatal:")
    machine_started = False
    for line_number, line in enumerate(lines, 1):
        lowered = line.lower()
        if machine_started and any(word in lowered for word in fatal_words):
            raise ValidationError(f"line {line_number}: crash or reboot evidence")
        record = parse_record(line, line_number)
        if record is None:
            continue
        machine_started = True
        if expected_sequence is None:
            expected_sequence = record["seq"]
        if record["seq"] != expected_sequence:
            raise ValidationError(f"line {line_number}: expected sequence {expected_sequence}, got {record['seq']}")
        expected_sequence += 1
        records.append(record)
    if not records:
        raise ValidationError("no VOICE_BENCH records")

    cases = {}
    case_order = []
    open_case = None
    results = {}
    previous_counters = None
    sample_rate_failed = False
    sample_criteria_failed = False
    web_last_advance_ms = [0] * len(WEB_LOAD_KEYS)
    for record in records:
        kind = record["kind"]
        if kind == "START":
            if open_case is not None:
                raise ValidationError(f"line {record['line']}: case {open_case['case']} was truncated")
            if record["case"] in cases:
                raise ValidationError(f"line {record['line']}: duplicate case {record['case']}")
            if record["mode"] not in MODES or record["filtered"] not in FILTERS:
                raise ValidationError(f"line {record['line']}: invalid mode/filter")
            if record["rate"] != SAMPLE_RATE_HZ or record["descriptor_frames"] != DESCRIPTOR_FRAMES:
                raise ValidationError(f"line {record['line']}: wrong 48 kHz descriptor geometry")
            if not 1 <= record["channels"] <= 64 or not 1 <= record["voices"] <= 256:
                raise ValidationError(f"line {record['line']}: capacity outside native IT limits")
            if record["phase"] not in ("qualify", "soak") or record["axis"] not in ("voices", "channels", "boundary"):
                raise ValidationError(f"line {record['line']}: invalid search phase/axis")
            bounded_value = record["voices"] if record["axis"] in ("voices", "boundary") else record["channels"]
            if not record["low"] <= bounded_value <= record["high"]:
                raise ValidationError(f"line {record['line']}: candidate is outside logged bounds")
            required_duration = long_seconds if record["phase"] == "soak" else short_seconds
            if record["duration_s"] < required_duration:
                raise ValidationError(f"line {record['line']}: case duration is too short")
            open_case = record
            previous_counters = None
            sample_rate_failed = False
            sample_criteria_failed = False
            web_last_advance_ms = [0] * len(WEB_LOAD_KEYS)
        elif kind == "SAMPLE":
            if open_case is None or record["case"] != open_case["case"]:
                raise ValidationError(f"line {record['line']}: sample outside its case")
            if previous_counters is not None:
                for key in COUNTER_KEYS:
                    if record[key] < previous_counters[key]:
                        raise ValidationError(f"line {record['line']}: {key} regressed")
            if not transport_rate_is_valid(record):
                sample_rate_failed = True
            if record["elapsed_ms"] >= VOICE_PLATEAU_SETTLE_MS and record["active"] != open_case["voices"]:
                sample_criteria_failed = True
            if open_case["mode"] == "web":
                if web_load_progress_failed(record, previous_counters, web_last_advance_ms):
                    sample_criteria_failed = True
            elif any(record[key] != 0 for key in WEB_LOAD_KEYS):
                raise ValidationError(f"line {record['line']}: audio case carries web-load evidence")
            if previous_counters is not None and record["heap_internal"] > previous_counters["heap_internal"]:
                raise ValidationError(f"line {record['line']}: heap low-water mark recovered")
            previous_counters = record
        elif kind == "END":
            if open_case is None or record["case"] != open_case["case"]:
                raise ValidationError(f"line {record['line']}: end outside its case")
            setup_rejection = record["status"] == "reject" and record["reason"] == "setup"
            criteria_rejection = record["status"] == "reject" and record["reason"] == "criteria"
            if record["status"] not in ("pass", "reject"):
                raise ValidationError(f"line {record['line']}: invalid case status")
            if (record["status"] == "pass" and record["reason"] != "none") or (record["status"] == "reject" and record["reason"] not in ("setup", "criteria")):
                raise ValidationError(f"line {record['line']}: invalid status/reason combination")
            if previous_counters is None and not setup_rejection:
                raise ValidationError(f"line {record['line']}: case has no periodic sample")
            if previous_counters is not None:
                for key in COUNTER_KEYS:
                    if record[key] < previous_counters[key]:
                        raise ValidationError(f"line {record['line']}: {key} regressed")
                if record["heap_internal"] > previous_counters["heap_internal"]:
                    raise ValidationError(f"line {record['line']}: heap low-water mark recovered")
                if open_case["mode"] == "web" and web_load_progress_failed(record, previous_counters, web_last_advance_ms):
                    sample_criteria_failed = True
            if open_case["mode"] == "audio" and any(record[key] != 0 for key in WEB_LOAD_KEYS):
                raise ValidationError(f"line {record['line']}: audio case carries web-load evidence")
            if open_case["mode"] == "web":
                try:
                    validate_host_web_load_fields(record, setup_rejection)
                except ValidationError as error:
                    raise ValidationError(f"line {record['line']}: {error}") from error
            elif any(key in record for key in HOST_WEB_LOAD_KEYS):
                raise ValidationError(f"line {record['line']}: audio case carries host web-load evidence")
            if not setup_rejection and record["elapsed_ms"] < open_case["duration_s"] * 1000:
                raise ValidationError(f"line {record['line']}: truncated case duration")
            if sample_rate_failed and not criteria_rejection:
                raise ValidationError(f"line {record['line']}: incorrect 48 kHz transport rate")
            passed = not sample_criteria_failed and case_passes(record, open_case)
            if (record["status"] == "pass") != passed:
                raise ValidationError(f"line {record['line']}: status disagrees with pass criteria")
            cases[open_case["case"]] = {"start": open_case, "end": record, "passed": passed}
            case_order.append(open_case["case"])
            open_case = None
            previous_counters = None
            sample_rate_failed = False
            sample_criteria_failed = False
            web_last_advance_ms = [0] * len(WEB_LOAD_KEYS)
        else:
            if open_case is not None:
                raise ValidationError(f"line {record['line']}: result before case end")
            key = (record["mode"], record["filtered"])
            if key in results:
                raise ValidationError(f"line {record['line']}: duplicate result for {key}")
            results[key] = record
    if open_case is not None:
        raise ValidationError(f"case {open_case['case']} was truncated")
    expected_results = {(mode, filtered) for mode in MODES for filtered in FILTERS}
    if set(results) != expected_results:
        raise ValidationError("missing final RESULT records for all four modes")
    for case in cases.values():
        case_key = (case["start"]["mode"], case["start"]["filtered"])
        if case["end"]["line"] >= results[case_key]["line"]:
            raise ValidationError(f"line {case['start']['line']}: case appears after its mode/filter RESULT")

    summaries = []
    for key in sorted(results):
        result = results[key]
        mode_case_names = [
            name for name in case_order
            if cases[name]["start"]["mode"] == key[0]
            and cases[name]["start"]["filtered"] == key[1]
        ]
        if not mode_case_names:
            raise ValidationError(f"line {result['line']}: result has no qualification search")
        case_index = 0

        def consume_case(expected, message):
            nonlocal case_index
            if case_index >= len(mode_case_names):
                raise ValidationError(f"line {result['line']}: missing {message}")
            name = mode_case_names[case_index]
            case_index += 1
            start = cases[name]["start"]
            actual = tuple(start[field] for field in ("mode", "filtered", "phase", "axis", "channels", "voices", "low", "high"))
            if actual != expected:
                raise ValidationError(f"line {start['line']}: incorrect {message}; expected {expected}, got {actual}")
            return name, cases[name]

        controller = SearchController()
        qualification_outcomes = {}
        qualification_names = {}
        while not controller.done:
            axis, low, high, channels, voices = controller.candidate()
            expected = (key[0], key[1], "qualify", axis, channels, voices, low, high)
            name, case = consume_case(expected, "next bisection candidate")
            qualification_outcomes[(channels, voices)] = case["passed"]
            qualification_names[(channels, voices)] = name
            controller.observe(case["passed"])
        if controller.best_pass is None:
            if not controller.has_zero_capacity_boundary():
                raise ValidationError(f"line {result['line']}: completed search has no passing capacity")
            reject_name = qualification_names[(1, 1)]
            if cases[reject_name]["end"]["reason"] != "criteria":
                raise ValidationError(f"line {result['line']}: zero capacity requires a criteria rejection at 1x1")
            if case_index != len(mode_case_names):
                extra = cases[mode_case_names[case_index]]["start"]
                raise ValidationError(f"line {extra['line']}: unexpected case after zero boundary converged")
            if (result["channel_limit"], result["voice_limit"]) != (1, 0):
                raise ValidationError(f"line {result['line']}: RESULT limit does not match the zero-capacity search")
            if result["pass_case"] != "none":
                raise ValidationError(f"line {result['line']}: zero-capacity result cannot name a passing case")
            if result["reject_case"] != reject_name or cases[reject_name]["passed"]:
                raise ValidationError(f"line {result['line']}: zero-capacity reject does not name the exact 1x1 qualification")
            summaries.append({
                "mode": key[0], "filtered": bool(key[1]), "channels": 1,
                "voices": 0, "pass_case": "none", "reject_case": reject_name,
            })
            continue

        channels = controller.channels
        provisional_voices = controller.best_pass
        expected = (key[0], key[1], "soak", "boundary", channels, provisional_voices, provisional_voices, provisional_voices)
        provisional_name, provisional_case = consume_case(expected, "provisional boundary soak")
        pass_name = provisional_name
        reject_name = "none"
        expected_limit = (channels, provisional_voices)

        if provisional_case["passed"]:
            rejection = controller.rejection_boundary()
            if rejection is not None:
                reject_axis, reject_channels, reject_voices = rejection
                reject_value = reject_channels if reject_axis == "channels" else reject_voices
                expected = (key[0], key[1], "soak", reject_axis, reject_channels, reject_voices, reject_value, reject_value)
                reject_name, reject_case = consume_case(expected, "adjacent reject boundary")
                if reject_case["passed"]:
                    raise ValidationError(f"line {reject_case['end']['line']}: adjacent rejected boundary passed its long soak")
            elif expected_limit != (64, 256):
                raise ValidationError(f"line {result['line']}: absent reject boundary below native maximum")
        else:
            upper_reject = provisional_voices
            reject_name = provisional_name
            lower_pass = None
            soaked_voices = {provisional_voices}
            while lower_pass is None or upper_reject != lower_pass + 1:
                candidate = next_stable_voice_candidate(
                    channels, lower_pass, upper_reject, qualification_outcomes, soaked_voices,
                )
                if candidate is None:
                    raise ValidationError(f"line {result['line']}: no capacity passes a long soak")
                candidate_voices, qualification_bounds = candidate
                if qualification_bounds is not None:
                    low, high = qualification_bounds
                    expected = (key[0], key[1], "qualify", "voices", channels, candidate_voices, low, high)
                    _, qualification_case = consume_case(expected, "stable-boundary qualification candidate")
                    qualification_outcomes[(channels, candidate_voices)] = qualification_case["passed"]
                expected = (
                    key[0], key[1], "soak", "boundary", channels, candidate_voices,
                    candidate_voices, candidate_voices,
                )
                candidate_name, candidate_case = consume_case(expected, "stable-boundary long soak")
                soaked_voices.add(candidate_voices)
                if candidate_case["passed"]:
                    lower_pass = candidate_voices
                    pass_name = candidate_name
                else:
                    upper_reject = candidate_voices
                    reject_name = candidate_name
            expected_limit = (channels, lower_pass)

        if case_index != len(mode_case_names):
            extra = cases[mode_case_names[case_index]]["start"]
            raise ValidationError(f"line {extra['line']}: unexpected case after stable boundary converged")
        actual_limit = (result["channel_limit"], result["voice_limit"])
        if actual_limit != expected_limit:
            raise ValidationError(f"line {result['line']}: RESULT limit does not match the completed search")
        if result["pass_case"] != pass_name or not cases[pass_name]["passed"]:
            raise ValidationError(f"line {result['line']}: result does not name its passing boundary")
        if result["reject_case"] != reject_name:
            raise ValidationError(f"line {result['line']}: reject boundary does not match the completed search")
        if reject_name != "none" and cases[reject_name]["passed"]:
            raise ValidationError(f"line {result['line']}: reject_case is not a completed rejecting soak")
        summaries.append({
            "mode": key[0], "filtered": bool(key[1]), "channels": result["channel_limit"],
            "voices": result["voice_limit"], "pass_case": pass_name, "reject_case": reject_name,
        })

    clean_records = [{key: value for key, value in record.items() if key != "line"} for record in records]
    clean_cases = {
        name: {
            "start": {key: value for key, value in case["start"].items() if key != "line"},
            "end": {key: value for key, value in case["end"].items() if key != "line"},
            "passed": case["passed"],
        }
        for name, case in sorted(cases.items())
    }
    return {"schema": SCHEMA_VERSION, "records": clean_records, "cases": clean_cases, "limits": summaries}


def format_record(record, sequence):
    """Format one parsed or host-produced record with a controller sequence."""
    fields = {key: value for key, value in record.items() if key not in ("line", "seq")}
    ordered = {"v": fields.pop("v", SCHEMA_VERSION), "seq": sequence, "kind": fields.pop("kind")}
    ordered.update(fields)
    return "VOICE_BENCH " + " ".join(f"{key}={value}" for key, value in ordered.items()) + "\n"


class WebLoadTracker:
    """Locked host-side request outcomes for one web candidate."""

    def __init__(self):
        self.lock = threading.Lock()
        self.counts = {kind: {"success": 0, "failure": 0} for kind in ("http", "upload", "ws")}

    def record(self, kind, succeeded):
        with self.lock:
            outcome = "success" if succeeded else "failure"
            self.counts[kind][outcome] += 1

    def snapshot(self):
        with self.lock:
            return {kind: dict(counts) for kind, counts in self.counts.items()}


def flatten_host_web_load(counts):
    return {
        f"host_{kind}_{'ok' if outcome == 'success' else 'fail'}": counts[kind][outcome]
        for kind in ("http", "upload", "ws") for outcome in ("success", "failure")
    }


def validate_host_web_load_fields(fields, setup_rejection):
    missing = [key for key in HOST_WEB_LOAD_KEYS if key not in fields]
    if missing:
        raise ValidationError(f"web case is missing host-load evidence {','.join(missing)}")
    if not setup_rejection and any(fields[key] == 0 for key in ("host_http_ok", "host_upload_ok", "host_ws_ok")):
        raise ValidationError("web case has no successful host workload activity")


def web_http_load(stop, board_ip, tracker):
    """Exercise reads and the upload parser without adopting another module."""
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    requests = [
        ("http", urllib.request.Request(f"http://{board_ip}/api/status")),
        ("http", urllib.request.Request(f"http://{board_ip}/api/modules")),
        ("http", urllib.request.Request(f"http://{board_ip}/api/upload-limits")),
        ("upload", urllib.request.Request(
            f"http://{board_ip}/api/modules", data=b"VOICE_BENCH_INVALID_UPLOAD",
            headers={"Content-Type": "application/octet-stream"}, method="POST",
        )),
    ]
    while not stop.is_set():
        for kind, request in requests:
            if stop.is_set():
                return
            try:
                with opener.open(request, timeout=5) as response:
                    response.read()
                    succeeded = 200 <= response.getcode() < 300
                tracker.record(kind, succeeded)
            except urllib.error.HTTPError as error:
                error.read()
                tracker.record(kind, kind == "upload" and 400 <= error.code < 500)
            except OSError:
                tracker.record(kind, False)


def web_socket_load(stop, board_ip, tracker):
    """Hold a production /ws telemetry consumer open, reconnecting after failures."""
    request = (
        f"GET /ws HTTP/1.1\r\nHost: {board_ip}\r\nUpgrade: websocket\r\n"
        "Connection: Upgrade\r\nSec-WebSocket-Key: c3RhcnBsYXllci1iZW5jaA==\r\n"
        "Sec-WebSocket-Version: 13\r\n\r\n"
    ).encode("ascii")
    while not stop.is_set():
        try:
            with socket.create_connection((board_ip, 80), timeout=5) as connection:
                connection.settimeout(2)
                connection.sendall(request)
                response = connection.recv(1024)
                if b" 101 " not in response.split(b"\r\n", 1)[0]:
                    tracker.record("ws", False)
                    continue
                while not stop.is_set():
                    try:
                        if not connection.recv(4096):
                            tracker.record("ws", False)
                            break
                        tracker.record("ws", True)
                    except TimeoutError:
                        tracker.record("ws", False)
                        continue
        except OSError:
            tracker.record("ws", False)


def parsed_case(lines, spec):
    """Strictly validate one reboot's UART stream and return its machine records."""
    fatal_words = ("panic", "backtrace", "watchdog", "brownout", "rst:", "fatal:")
    records = []
    machine_started = False
    for line_number, line in enumerate(lines, 1):
        if machine_started and any(word in line.lower() for word in fatal_words):
            raise ValidationError(f"{spec.name}: crash or reboot evidence")
        record = parse_record(line, line_number)
        if record is not None:
            machine_started = True
            records.append(record)
    if len(records) < 2 or records[0]["kind"] != "START" or records[-1]["kind"] != "END":
        raise ValidationError(f"{spec.name}: missing START or END")
    for sequence, record in enumerate(records):
        if record["seq"] != sequence:
            raise ValidationError(f"{spec.name}: expected UART sequence {sequence}, got {record['seq']}")
        if record.get("case") != spec.name:
            raise ValidationError(f"{spec.name}: UART record names another case")
    start = records[0]
    expected = (spec.mode, spec.filtered, spec.channels, spec.voices, spec.duration_s, spec.phase, spec.axis, spec.low, spec.high)
    actual = tuple(start[key] for key in ("mode", "filtered", "channels", "voices", "duration_s", "phase", "axis", "low", "high"))
    if actual != expected:
        raise ValidationError(f"{spec.name}: firmware metadata does not match the requested build")
    if start["rate"] != SAMPLE_RATE_HZ or start["descriptor_frames"] != DESCRIPTOR_FRAMES:
        raise ValidationError(f"{spec.name}: wrong 48 kHz descriptor geometry")
    previous = None
    sample_rate_failed = False
    sample_criteria_failed = False
    web_last_advance_ms = [0] * len(WEB_LOAD_KEYS)
    for record in records[1:-1]:
        if record["kind"] != "SAMPLE":
            raise ValidationError(f"{spec.name}: non-periodic record inside case")
        if previous is not None:
            for key in COUNTER_KEYS:
                if record[key] < previous[key]:
                    raise ValidationError(f"{spec.name}: {key} regressed")
        if not transport_rate_is_valid(record):
            sample_rate_failed = True
        if record["elapsed_ms"] >= VOICE_PLATEAU_SETTLE_MS and record["active"] != start["voices"]:
            sample_criteria_failed = True
        if start["mode"] == "web":
            if web_load_progress_failed(record, previous, web_last_advance_ms):
                sample_criteria_failed = True
        elif any(record[key] != 0 for key in WEB_LOAD_KEYS):
            raise ValidationError(f"{spec.name}: audio case carries web-load evidence")
        if previous is not None and record["heap_internal"] > previous["heap_internal"]:
            raise ValidationError(f"{spec.name}: heap low-water mark recovered")
        previous = record
    end = records[-1]
    setup_rejection = end["status"] == "reject" and end["reason"] == "setup"
    criteria_rejection = end["status"] == "reject" and end["reason"] == "criteria"
    if end["status"] not in ("pass", "reject"):
        raise ValidationError(f"{spec.name}: invalid case status")
    if (end["status"] == "pass" and end["reason"] != "none") or (end["status"] == "reject" and end["reason"] not in ("setup", "criteria")):
        raise ValidationError(f"{spec.name}: invalid status/reason combination")
    if previous is None and not setup_rejection:
        raise ValidationError(f"{spec.name}: case has no periodic sample")
    if previous is not None:
        for key in COUNTER_KEYS:
            if end[key] < previous[key]:
                raise ValidationError(f"{spec.name}: {key} regressed")
        if end["heap_internal"] > previous["heap_internal"]:
            raise ValidationError(f"{spec.name}: heap low-water mark recovered")
        if start["mode"] == "web" and web_load_progress_failed(end, previous, web_last_advance_ms):
            sample_criteria_failed = True
    if start["mode"] == "audio" and any(end[key] != 0 for key in WEB_LOAD_KEYS):
        raise ValidationError(f"{spec.name}: audio case carries web-load evidence")
    if not setup_rejection and end["elapsed_ms"] < spec.duration_s * 1000:
        raise ValidationError(f"{spec.name}: truncated case duration")
    if sample_rate_failed and not criteria_rejection:
        raise ValidationError(f"{spec.name}: incorrect 48 kHz transport rate")
    if (end["status"] == "pass") != (not sample_criteria_failed and case_passes(end, start)):
        raise ValidationError(f"{spec.name}: status disagrees with pass criteria")
    return records


def require_host_web_load(spec, counts, setup_rejection):
    """Require each host workload class to receive a response during a timed web case."""
    if spec.mode != "web" or setup_rejection:
        return
    missing = [kind for kind, outcomes in counts.items() if outcomes["success"] == 0]
    if missing:
        details = ", ".join(f"{kind}={counts[kind]['success']}/{counts[kind]['failure']}" for kind in counts)
        raise ValidationError(f"{spec.name}: web load has no successful {','.join(missing)} activity ({details})")


class HardwareRunner:
    """Build, flash and capture one compile-time-sized candidate per reboot."""

    def __init__(self, arguments):
        self.arguments = arguments
        self.artifact_dir = arguments.artifact_dir
        self.artifact_dir.mkdir(parents=True, exist_ok=True)

    def __call__(self, spec):
        raw_path = self.artifact_dir / f"{spec.name}.uart.log"
        load_path = self.artifact_dir / f"{spec.name}.web-load.json"
        if self.arguments.resume and raw_path.exists():
            lines = raw_path.read_text(encoding="utf-8", errors="replace").splitlines(keepends=True)
            records = parsed_case(lines, spec)
            if spec.mode == "web":
                if not load_path.exists():
                    raise ValidationError(f"{spec.name}: resume artifact is missing host web-load evidence")
                try:
                    load_evidence = json.loads(load_path.read_text(encoding="utf-8"))
                except (OSError, ValueError) as error:
                    raise ValidationError(f"{spec.name}: resume host web-load evidence is malformed") from error
                if load_evidence.get("spec") != spec._asdict() or set(load_evidence.get("counts", {})) != set(HOST_WEB_LOAD_KEYS):
                    raise ValidationError(f"{spec.name}: resume host web-load evidence does not match the requested case")
                counts = load_evidence["counts"]
                if any(not isinstance(value, int) or value < 0 for value in counts.values()):
                    raise ValidationError(f"{spec.name}: resume host web-load counts are invalid")
                setup_rejection = records[-1]["status"] == "reject" and records[-1]["reason"] == "setup"
                validate_host_web_load_fields(counts, setup_rejection)
                records[-1].update(counts)
            return records

        environment = os.environ.copy()
        features = ["voice-bench"]
        if spec.mode == "web":
            features.append("web")
        if spec.filtered:
            features.append("voice-bench-filtered")
        environment.update({
            "STARPLAYER_FEATURES": ",".join(features),
            "STARPLAYER_RFC2217_ENDPOINT": self.arguments.serial_endpoint,
            "STARPLAYER_VOICE_BENCH_CHANNELS": str(spec.channels),
            "STARPLAYER_VOICE_BENCH_VOICES": str(spec.voices),
            "STARPLAYER_VOICE_BENCH_SECONDS": str(spec.duration_s),
            "STARPLAYER_VOICE_BENCH_PHASE": spec.phase,
            "STARPLAYER_VOICE_BENCH_AXIS": spec.axis,
            "STARPLAYER_VOICE_BENCH_LOW": str(spec.low),
            "STARPLAYER_VOICE_BENCH_HIGH": str(spec.high),
            "STARPLAYER_VOICE_BENCH_CASE": spec.name,
            "RUST_MIN_STACK": str(RUST_MIN_STACK_BYTES),
        })
        build_log = self.artifact_dir / f"{spec.name}.build.log"
        completed = subprocess.run(
            [str(self.arguments.flash_command)], cwd=self.arguments.repository / "embedded",
            env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            check=False,
        )
        build_log.write_text(completed.stdout, encoding="utf-8")
        if completed.returncode:
            raise ValidationError(f"{spec.name}: image build or owner flash command failed; see {build_log}")

        lines = self.capture(spec, raw_path)
        records = parsed_case(lines, spec)
        if spec.mode == "web":
            counts = flatten_host_web_load(self.host_web_load_counts)
            records[-1].update(counts)
            load_path.write_text(json.dumps({"spec": spec._asdict(), "counts": counts}, sort_keys=True) + "\n", encoding="utf-8")
        return records

    def capture(self, spec, raw_path=None):
        try:
            import serial
        except ImportError as error:
            raise SystemExit("run mode requires pyserial (python3 -m pip install pyserial)") from error
        stop = threading.Event()
        load_threads = []
        load_tracker = WebLoadTracker()
        if spec.mode == "web":
            if not self.arguments.board_ip:
                raise ValidationError("web mode requires --board-ip")
            for function in (web_http_load, web_http_load, web_socket_load):
                thread = threading.Thread(target=function, args=(stop, self.arguments.board_ip, load_tracker), daemon=True)
                thread.start()
                load_threads.append(thread)
        lines = []
        raw_file = raw_path.open("w", encoding="utf-8") if raw_path is not None else None
        try:
            with serial.serial_for_url(self.arguments.serial_endpoint, baudrate=UART_BAUD, timeout=1) as uart:
                # This runner attaches after esptool has closed the RFC2217 connection.
                # Pulse the bridge's reset line only here, before the firmware's capture
                # grace and the benchmark deadline begin.
                uart.dtr = False
                time.sleep(RESET_PULSE_SECONDS)
                uart.rts = True
                time.sleep(RESET_PULSE_SECONDS)
                uart.rts = False
                deadline = time.monotonic() + spec.duration_s + self.arguments.startup_seconds
                machine_started = False
                expected_sequence = 0
                final_record = None
                while time.monotonic() < deadline:
                    data = uart.readline()
                    if not data:
                        continue
                    line = data.decode("utf-8", errors="replace")
                    lines.append(line)
                    if raw_file is not None:
                        raw_file.write(line)
                        raw_file.flush()
                    if machine_started and any(word in line.lower() for word in ("panic", "backtrace", "watchdog", "brownout", "rst:", "fatal:")):
                        raise ValidationError(f"{spec.name}: crash or reboot evidence")
                    record = parse_record(line, len(lines))
                    if record is not None:
                        machine_started = True
                        if record["seq"] != expected_sequence:
                            raise ValidationError(f"{spec.name}: expected UART sequence {expected_sequence}, got {record['seq']}")
                        expected_sequence += 1
                        if record.get("case") != spec.name:
                            raise ValidationError(f"{spec.name}: UART record names another case")
                        if record["kind"] == "END":
                            final_record = record
                            break
        finally:
            stop.set()
            for thread in load_threads:
                thread.join(timeout=5)
            if raw_file is not None:
                raw_file.close()
        setup_rejection = final_record is not None and final_record.get("status") == "reject" and final_record.get("reason") == "setup"
        self.host_web_load_counts = load_tracker.snapshot()
        require_host_web_load(spec, self.host_web_load_counts, setup_rejection)
        return lines


def setup_reject_records(spec, reason):
    """Fixture for the structured rejection emitted by firmware setup failures."""
    start = {
        "v": SCHEMA_VERSION, "seq": 0, "kind": "START", "case": spec.name,
        "mode": spec.mode, "filtered": spec.filtered, "channels": spec.channels,
        "voices": spec.voices, "rate": SAMPLE_RATE_HZ, "descriptor_frames": DESCRIPTOR_FRAMES,
        "duration_s": spec.duration_s, "phase": spec.phase, "axis": spec.axis,
        "low": spec.low, "high": spec.high, "dma_base": 0,
    }
    end = {
        "v": SCHEMA_VERSION, "seq": 1, "kind": "END", "case": spec.name,
        "elapsed_ms": 0, "frames": 0, "active": 0, "peak_active": 0,
        "render_max_us": 0, "render_p50_us": 0, "render_p95_us": 0,
        "misses": 0, "underruns": 0, "dma_errors": 0, "warnings": 0,
        "steals": 0, "heap_internal": 0, "heap_external": 0,
        "web_http_ok": 0, "web_upload_ok": 0, "web_ws_ok": 0,
        "status": "reject", "reason": reason,
    }
    return [start, end]


def execute_search(short_seconds, long_seconds, execute_case, case_observer=None):
    """Run all four searches and return one normalized, globally sequenced proof."""
    normalized = []
    sequence = 0

    def append_records(records):
        nonlocal sequence
        for record in records:
            normalized.append(format_record(record, sequence))
            sequence += 1

    for mode in MODES:
        for filtered in FILTERS:
            controller = SearchController()
            qualification_index = 0
            qualification_outcomes = {}
            qualification_names = {}
            qualification_reasons = {}
            while not controller.done:
                axis, low, high, channels, voices = controller.candidate()
                name = f"{mode}-f{filtered}-q{qualification_index}-{axis}-{channels}x{voices}"
                spec = CaseSpec(name, mode, filtered, channels, voices, short_seconds, "qualify", axis, low, high)
                records = execute_case(spec)
                passed = records[-1]["status"] == "pass"
                append_records(records)
                if case_observer:
                    case_observer(spec, passed)
                qualification_outcomes[(channels, voices)] = passed
                qualification_names[(channels, voices)] = name
                qualification_reasons[(channels, voices)] = records[-1]["reason"]
                controller.observe(passed)
                qualification_index += 1
            if controller.best_pass is None:
                if not controller.has_zero_capacity_boundary():
                    raise ValidationError(f"{mode} filtered={filtered}: no capacity passes the minimum search")
                if qualification_reasons[(1, 1)] != "criteria":
                    raise ValidationError(f"{mode} filtered={filtered}: zero capacity requires a criteria rejection at 1x1")
                result = {
                    "v": SCHEMA_VERSION, "kind": "RESULT", "mode": mode, "filtered": filtered,
                    "channel_limit": 1, "voice_limit": 0,
                    "pass_case": "none", "reject_case": qualification_names[(1, 1)],
                }
                normalized.append(format_record(result, sequence))
                sequence += 1
                continue

            channels = controller.channels
            voices = controller.best_pass
            pass_name = f"{mode}-f{filtered}-soak-pass-{channels}x{voices}"
            pass_spec = CaseSpec(pass_name, mode, filtered, channels, voices, long_seconds, "soak", "boundary", voices, voices)
            pass_records = execute_case(pass_spec)
            append_records(pass_records)
            if case_observer:
                case_observer(pass_spec, pass_records[-1]["status"] == "pass")

            reject_name = "none"
            if pass_records[-1]["status"] == "pass":
                rejection = controller.rejection_boundary()
                if rejection is not None:
                    reject_axis, reject_channels, reject_voices = rejection
                    reject_value = reject_channels if reject_axis == "channels" else reject_voices
                    reject_name = f"{mode}-f{filtered}-soak-reject-{reject_channels}x{reject_voices}"
                    reject_spec = CaseSpec(reject_name, mode, filtered, reject_channels, reject_voices, long_seconds, "soak", reject_axis, reject_value, reject_value)
                    reject_records = execute_case(reject_spec)
                    if reject_records[-1]["status"] == "pass":
                        raise ValidationError(f"{reject_name}: adjacent rejected boundary passed its long soak")
                    append_records(reject_records)
                    if case_observer:
                        case_observer(reject_spec, False)
            else:
                upper_reject = voices
                reject_name = pass_name
                lower_pass = None
                soaked_voices = {voices}
                while lower_pass is None or upper_reject != lower_pass + 1:
                    candidate = next_stable_voice_candidate(
                        channels, lower_pass, upper_reject, qualification_outcomes, soaked_voices,
                    )
                    if candidate is None:
                        raise ValidationError(f"{mode} filtered={filtered}: no capacity passes a long soak")
                    candidate_voices, qualification_bounds = candidate
                    if qualification_bounds is not None:
                        low, high = qualification_bounds
                        name = f"{mode}-f{filtered}-q{qualification_index}-voices-{channels}x{candidate_voices}"
                        spec = CaseSpec(name, mode, filtered, channels, candidate_voices, short_seconds, "qualify", "voices", low, high)
                        records = execute_case(spec)
                        qualified = records[-1]["status"] == "pass"
                        append_records(records)
                        if case_observer:
                            case_observer(spec, qualified)
                        qualification_outcomes[(channels, candidate_voices)] = qualified
                        qualification_index += 1
                    candidate_name = f"{mode}-f{filtered}-soak-pass-{channels}x{candidate_voices}"
                    candidate_spec = CaseSpec(
                        candidate_name, mode, filtered, channels, candidate_voices,
                        long_seconds, "soak", "boundary", candidate_voices, candidate_voices,
                    )
                    candidate_records = execute_case(candidate_spec)
                    candidate_passed = candidate_records[-1]["status"] == "pass"
                    append_records(candidate_records)
                    if case_observer:
                        case_observer(candidate_spec, candidate_passed)
                    soaked_voices.add(candidate_voices)
                    if candidate_passed:
                        lower_pass = candidate_voices
                        voices = candidate_voices
                        pass_name = candidate_name
                    else:
                        upper_reject = candidate_voices
                        reject_name = candidate_name

            result = {
                "v": SCHEMA_VERSION, "kind": "RESULT", "mode": mode, "filtered": filtered,
                "channel_limit": channels, "voice_limit": voices,
                "pass_case": pass_name, "reject_case": reject_name,
            }
            normalized.append(format_record(result, sequence))
            sequence += 1
    return normalized


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="mode", required=True)
    run = subparsers.add_parser("run")
    run.add_argument("--serial-endpoint", default="rfc2217://192.168.0.151:8086?ign_set_control")
    run.add_argument("--board-ip")
    run.add_argument("--short-seconds", type=int, default=60)
    run.add_argument("--long-seconds", type=int, default=600)
    run.add_argument("--startup-seconds", type=int, default=90)
    run.add_argument("--raw-log", type=Path, default=Path("a1s-voice-bench.log"))
    run.add_argument("--artifact-dir", type=Path)
    run.add_argument("--resume", action="store_true")
    run.add_argument("--json-report", type=Path, default=Path("a1s-voice-bench.json"))
    run.add_argument("--repository", type=Path, default=Path(__file__).resolve().parents[2])
    run.add_argument("--flash-command", type=Path)
    verify = subparsers.add_parser("verify")
    verify.add_argument("logs", nargs="+", type=Path)
    verify.add_argument("--short-seconds", type=int, default=60)
    verify.add_argument("--long-seconds", type=int, default=600)
    verify.add_argument("--json-report", type=Path, default=Path("a1s-voice-bench.json"))
    arguments = parser.parse_args(argv)

    if arguments.mode == "run":
        if not arguments.board_ip:
            print("FAIL: run mode requires --board-ip for the web-loaded cases")
            return 1
        if arguments.artifact_dir is None:
            arguments.artifact_dir = arguments.raw_log.with_suffix(arguments.raw_log.suffix + ".d")
        if arguments.flash_command is None:
            arguments.flash_command = arguments.repository / "embedded" / "flash_image.sh"
        runner = HardwareRunner(arguments)
        def report_case(spec, passed):
            print(f"{'PASS' if passed else 'REJECT'} {spec.name}")
        try:
            lines = execute_search(arguments.short_seconds, arguments.long_seconds, runner, report_case)
        except ValidationError as error:
            print(f"FAIL: {error}")
            return 1
        arguments.raw_log.write_text("".join(lines), encoding="utf-8")
    else:
        lines = []
        for path in arguments.logs:
            lines.extend(path.read_text(encoding="utf-8", errors="replace").splitlines(keepends=True))
    try:
        report = validate_transcript(lines, arguments.short_seconds, arguments.long_seconds)
    except ValidationError as error:
        print(f"FAIL: {error}")
        return 1
    arguments.json_report.write_text(json.dumps(report, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    for limit in report["limits"]:
        print(f"PASS {limit['mode']} filtered={int(limit['filtered'])}: {limit['channels']} channels / {limit['voices']} voices")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
