import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import threading
import types
import unittest
from unittest import mock


SCRIPT = Path(__file__).with_name("a1s_voice_bench.py")
SPEC = importlib.util.spec_from_file_location("a1s_voice_bench", SCRIPT)
bench = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench)


def valid_transcript():
    sequence = 0
    lines = ["human boot diagnostic\n"]

    def emit(kind, fields):
        nonlocal sequence
        values = {"v": 1, "seq": sequence, "kind": kind, **fields}
        sequence += 1
        lines.append("VOICE_BENCH " + " ".join(f"{key}={value}" for key, value in values.items()) + "\n")

    for mode in bench.MODES:
        for filtered in bench.FILTERS:
            prefix = f"{mode}-f{filtered}"
            for phase, duration, name in (("qualify", 60, prefix + "-q"), ("soak", 600, prefix + "-pass")):
                emit("START", {
                    "case": name, "mode": mode, "filtered": filtered, "channels": 64, "voices": 256,
                    "rate": 48000, "descriptor_frames": 256, "duration_s": duration,
                    "phase": phase, "axis": "voices" if phase == "qualify" else "boundary",
                    "low": 32 if phase == "qualify" else 256, "high": 256, "dma_base": 2,
                })
                for elapsed in (duration * 500, duration * 1000):
                    web_count = elapsed // 1000 if mode == "web" else 0
                    emit("SAMPLE", {
                        "case": name, "elapsed_ms": elapsed, "frames": elapsed * 48, "active": 256,
                        "peak_active": 256, "render_max_us": 3200, "render_p50_us": 2500,
                        "render_p95_us": 3000, "misses": 0, "underruns": 0, "dma_errors": 2,
                        "warnings": 0, "steals": elapsed // 10, "heap_internal": 12000,
                        "heap_external": 2000000,
                        "web_http_ok": web_count, "web_upload_ok": web_count, "web_ws_ok": web_count,
                    })
                web_count = duration + 1 if mode == "web" else 0
                emit("END", {
                    "case": name, "elapsed_ms": duration * 1000, "frames": duration * 48000,
                    "active": 256, "peak_active": 256, "render_max_us": 3200,
                    "render_p50_us": 2500, "render_p95_us": 3000, "misses": 0,
                    "underruns": 0, "dma_errors": 2, "warnings": 0, "steals": duration * 100,
                    "heap_internal": 12000, "heap_external": 2000000,
                    "web_http_ok": web_count, "web_upload_ok": web_count, "web_ws_ok": web_count,
                    **({
                        "host_http_ok": 10, "host_http_fail": 0,
                        "host_upload_ok": 10, "host_upload_fail": 0,
                        "host_ws_ok": 10, "host_ws_fail": 0,
                    } if mode == "web" else {}),
                    "status": "pass", "reason": "none",
                })
            emit("RESULT", {
                "mode": mode, "filtered": filtered, "channel_limit": 64, "voice_limit": 256,
                "pass_case": prefix + "-pass", "reject_case": "none",
            })
    return lines


def simulated_case_records(spec, passed):
    start = {
        "v": 1, "seq": 0, "kind": "START", "case": spec.name,
        "mode": spec.mode, "filtered": spec.filtered, "channels": spec.channels,
        "voices": spec.voices, "rate": 48000, "descriptor_frames": 256,
        "duration_s": spec.duration_s, "phase": spec.phase, "axis": spec.axis,
        "low": spec.low, "high": spec.high, "dma_base": 2,
    }
    sample = {
        "v": 1, "seq": 1, "kind": "SAMPLE", "case": spec.name,
        "elapsed_ms": spec.duration_s * 1000,
        "frames": spec.duration_s * 48000 if passed else 128,
        "active": spec.voices if passed else 1, "peak_active": spec.voices if passed else 1,
        "render_max_us": 3200 if passed else 42337, "render_p50_us": 2200,
        "render_p95_us": 3200, "misses": 0 if passed else 1, "underruns": 0,
        "dma_errors": 2 if passed else 953, "warnings": 0, "steals": 100,
        "heap_internal": 12000, "heap_external": 2000000,
        "web_http_ok": 1 if spec.mode == "web" else 0,
        "web_upload_ok": 1 if spec.mode == "web" else 0,
        "web_ws_ok": 1 if spec.mode == "web" else 0,
    }
    end = {
        **sample, "seq": 2, "kind": "END",
        "web_http_ok": 2 if spec.mode == "web" else 0,
        "web_upload_ok": 2 if spec.mode == "web" else 0,
        "web_ws_ok": 2 if spec.mode == "web" else 0,
        **({
            "host_http_ok": 2, "host_http_fail": 0,
            "host_upload_ok": 2, "host_upload_fail": 0,
            "host_ws_ok": 2, "host_ws_fail": 0,
        } if spec.mode == "web" else {}),
        "status": "pass" if passed else "reject",
        "reason": "none" if passed else "criteria",
    }
    return [start, sample, end]


class SearchTests(unittest.TestCase):
    def test_voice_bisection_selects_exact_boundary(self):
        controller = bench.SearchController()
        seen = []
        while not controller.done:
            _, _, _, channels, voices = controller.candidate()
            seen.append((channels, voices))
            controller.observe(voices <= 173)
        self.assertEqual(controller.best_pass, 173)
        self.assertEqual(controller.best_reject, 174)
        self.assertLess(len(seen), 12)

    def test_channel_fallback_then_repeats_voice_search(self):
        controller = bench.SearchController()
        while not controller.done:
            _, _, _, channels, voices = controller.candidate()
            passed = channels <= 40 and voices <= 87
            controller.observe(passed)
        self.assertEqual(controller.channels, 40)
        self.assertEqual(controller.best_pass, 87)
        self.assertEqual(controller.best_reject, 88)

    def test_channel_reject_survives_when_the_second_voice_search_reaches_maximum(self):
        controller = bench.SearchController()
        while not controller.done:
            _, _, _, channels, _ = controller.candidate()
            controller.observe(channels <= 40)
        self.assertEqual((controller.channels, controller.best_pass, controller.best_reject), (40, 256, None))
        self.assertEqual(controller.rejection_boundary(), ("channels", 41, 32))

    def test_one_channel_fallback_reports_limit_below_thirty_two_voices(self):
        for voice_limit in (1, 17, 31):
            with self.subTest(voice_limit=voice_limit):
                controller = bench.SearchController()
                seen = []
                while not controller.done:
                    axis, low, high, channels, voices = controller.candidate()
                    seen.append((axis, low, high, channels, voices))
                    controller.observe(channels == 1 and voices <= voice_limit)
                self.assertEqual(seen[0], ("voices", 32, 256, 64, 256))
                self.assertIn(("voices", 1, 31, 1, 31), seen)
                self.assertEqual(controller.channels, 1)
                self.assertEqual(controller.best_pass, voice_limit)
                self.assertEqual(controller.best_reject, voice_limit + 1)

    def test_exact_one_by_one_rejection_completes_at_zero_capacity(self):
        controller = bench.SearchController()
        seen = []
        while not controller.done:
            candidate = controller.candidate()
            seen.append(candidate)
            controller.observe(False)
        self.assertEqual(seen[-1], ("voices", 1, 1, 1, 1))
        self.assertIsNone(controller.best_pass)
        self.assertEqual(controller.best_reject, 1)
        self.assertTrue(controller.has_zero_capacity_boundary())

    def test_end_to_end_driver_runs_all_builds_and_adjacent_soaks(self):
        calls = []
        limits = {
            ("audio", 0): 17,
            ("audio", 1): None,
            ("web", 0): 121,
            ("web", 1): 93,
        }

        def execute(spec):
            calls.append(spec)
            limit = limits[(spec.mode, spec.filtered)]
            passed = spec.channels <= 40 if limit is None else spec.voices <= limit
            return simulated_case_records(spec, passed)

        lines = bench.execute_search(1, 2, execute)
        report = bench.validate_transcript(lines, short_seconds=1, long_seconds=2)
        self.assertEqual(len(report["limits"]), 4)
        self.assertEqual(report["limits"][0]["channels"], 1)
        self.assertEqual(report["limits"][0]["voices"], 17)
        self.assertEqual((report["limits"][1]["channels"], report["limits"][1]["voices"]), (40, 256))
        for mode in bench.MODES:
            for filtered in bench.FILTERS:
                soaks = [spec for spec in calls if spec.mode == mode and spec.filtered == filtered and spec.phase == "soak"]
                if (mode, filtered) == ("audio", 1):
                    self.assertEqual([(spec.axis, spec.channels, spec.voices) for spec in soaks], [
                        ("boundary", 40, 256), ("channels", 41, 32),
                    ])
                else:
                    limit = limits[(mode, filtered)]
                    self.assertEqual([(spec.channels, spec.voices) for spec in soaks], [
                        (1 if limit == 17 else 64, limit), (1 if limit == 17 else 64, limit + 1),
                    ])

        wrong_reject = list(lines)
        for index, line in enumerate(wrong_reject):
            if "kind=RESULT mode=audio filtered=1" in line:
                wrong_reject[index] = line.replace("reject_case=audio-f1-soak-reject-41x32", "reject_case=audio-f0-soak-reject-1x18")
                break
        with self.assertRaisesRegex(bench.ValidationError, "reject boundary does not match"):
            bench.validate_transcript(wrong_reject, short_seconds=1, long_seconds=2)

    def test_failed_provisional_soak_steps_down_to_qualified_adjacent_pass(self):
        calls = []

        def execute(spec):
            calls.append(spec)
            limit = 3 if spec.phase == "qualify" else 2
            return simulated_case_records(spec, spec.voices <= limit)

        lines = bench.execute_search(1, 2, execute)
        report = bench.validate_transcript(lines, short_seconds=1, long_seconds=2)
        self.assertTrue(all((limit["channels"], limit["voices"]) == (1, 2) for limit in report["limits"]))
        first = report["limits"][0]
        self.assertEqual(first["pass_case"], "audio-f0-soak-pass-1x2")
        self.assertEqual(first["reject_case"], "audio-f0-soak-pass-1x3")
        audio_soaks = [spec for spec in calls if spec.mode == "audio" and spec.filtered == 0 and spec.phase == "soak"]
        self.assertEqual([(spec.voices, spec.name) for spec in audio_soaks], [
            (3, "audio-f0-soak-pass-1x3"),
            (2, "audio-f0-soak-pass-1x2"),
        ])

        forged_result = [
            line.replace("reject_case=audio-f0-soak-pass-1x3", "reject_case=audio-f0-soak-pass-1x2")
            if "kind=RESULT mode=audio filtered=0" in line else line
            for line in lines
        ]
        with self.assertRaisesRegex(bench.ValidationError, "reject boundary does not match"):
            bench.validate_transcript(forged_result, short_seconds=1, long_seconds=2)

        forged_tuple = [
            line.replace("voices=3", "voices=4").replace("low=3 high=3", "low=4 high=4")
            if "kind=START case=audio-f0-soak-pass-1x3" in line else line
            for line in lines
        ]
        with self.assertRaisesRegex(bench.ValidationError, "provisional boundary soak"):
            bench.validate_transcript(forged_tuple, short_seconds=1, long_seconds=2)

    def test_failed_maximum_soak_uses_bounded_extra_qualification_search(self):
        calls = []

        def execute(spec):
            calls.append(spec)
            passed = True if spec.phase == "qualify" else spec.voices <= 200
            return simulated_case_records(spec, passed)

        lines = bench.execute_search(1, 2, execute)
        report = bench.validate_transcript(lines, short_seconds=1, long_seconds=2)
        self.assertTrue(all((limit["channels"], limit["voices"]) == (64, 200) for limit in report["limits"]))
        audio_qualifications = [
            spec for spec in calls if spec.mode == "audio" and spec.filtered == 0 and spec.phase == "qualify"
        ]
        self.assertEqual((audio_qualifications[1].low, audio_qualifications[1].high, audio_qualifications[1].voices), (1, 255, 128))
        self.assertLess(len(audio_qualifications), 12)
        first = report["limits"][0]
        self.assertEqual((first["pass_case"], first["reject_case"]), (
            "audio-f0-soak-pass-64x200", "audio-f0-soak-pass-64x201",
        ))

    def test_zero_capacity_result_uses_exact_rejecting_one_by_one_qualification(self):
        calls = []

        def execute(spec):
            calls.append(spec)
            limit = 0 if (spec.mode, spec.filtered) == ("web", 1) else 3
            return simulated_case_records(spec, spec.voices <= limit)

        lines = bench.execute_search(1, 2, execute)
        report = bench.validate_transcript(lines, short_seconds=1, long_seconds=2)
        zero = next(limit for limit in report["limits"] if (limit["mode"], limit["filtered"]) == ("web", True))
        one_by_one = next(
            spec for spec in calls
            if (spec.mode, spec.filtered, spec.channels, spec.voices) == ("web", 1, 1, 1)
        )
        self.assertEqual((zero["channels"], zero["voices"], zero["pass_case"], zero["reject_case"]), (
            1, 0, "none", one_by_one.name,
        ))
        self.assertFalse(any(spec.mode == "web" and spec.filtered == 1 and spec.phase == "soak" for spec in calls))

        wrong_reject = [
            line.replace(f"reject_case={one_by_one.name}", "reject_case=web-f1-q19-voices-1x2")
            if "kind=RESULT mode=web filtered=1" in line else line
            for line in lines
        ]
        with self.assertRaisesRegex(bench.ValidationError, "exact 1x1 qualification"):
            bench.validate_transcript(wrong_reject, short_seconds=1, long_seconds=2)

        invented_pass = [
            line.replace("pass_case=none", f"pass_case={one_by_one.name}")
            if "kind=RESULT mode=web filtered=1" in line else line
            for line in lines
        ]
        with self.assertRaisesRegex(bench.ValidationError, "cannot name a passing case"):
            bench.validate_transcript(invented_pass, short_seconds=1, long_seconds=2)

        forged_setup = [
            line.replace("frames=128", "frames=48000").replace("reason=criteria", "reason=setup")
            if f"case={one_by_one.name}" in line else line
            for line in lines
        ]
        with self.assertRaisesRegex(bench.ValidationError, "criteria rejection at 1x1"):
            bench.validate_transcript(forged_setup, short_seconds=1, long_seconds=2)

    def test_one_by_one_setup_rejection_aborts_instead_of_reporting_zero(self):
        def execute(spec):
            if (spec.channels, spec.voices) == (1, 1):
                return bench.setup_reject_records(spec, "setup")
            return simulated_case_records(spec, False)

        with self.assertRaisesRegex(bench.ValidationError, "criteria rejection at 1x1"):
            bench.execute_search(1, 2, execute)


class TranscriptTests(unittest.TestCase):
    def web_settle_case(self, first_counts, settled_counts, end_counts=None, settled_elapsed=None, end_elapsed=6000):
        settled_elapsed = bench.WEB_LOAD_SETTLE_MS if settled_elapsed is None else settled_elapsed
        duration_s = end_elapsed // 1000
        spec = bench.CaseSpec("web-settle", "web", 0, 1, 1, duration_s, "qualify", "voices", 1, 1)
        start = {
            "v": 1, "seq": 0, "kind": "START", "case": spec.name,
            "mode": spec.mode, "filtered": spec.filtered, "channels": spec.channels,
            "voices": spec.voices, "rate": 48000, "descriptor_frames": 256,
            "duration_s": spec.duration_s, "phase": spec.phase, "axis": spec.axis,
            "low": spec.low, "high": spec.high, "dma_base": 2,
        }

        def sample(sequence, kind, elapsed_ms, counts):
            record = {
                "v": 1, "seq": sequence, "kind": kind, "case": spec.name,
                "elapsed_ms": elapsed_ms, "frames": elapsed_ms * 48, "active": 1,
                "peak_active": 1, "render_max_us": 3200, "render_p50_us": 2500,
                "render_p95_us": 3000, "misses": 0, "underruns": 0, "dma_errors": 2,
                "warnings": 0, "steals": sequence, "heap_internal": 12000,
                "heap_external": 2000000, "web_http_ok": counts[0],
                "web_upload_ok": counts[1], "web_ws_ok": counts[2],
            }
            if kind == "END":
                record.update(status="pass", reason="none")
            return record

        if end_counts is None:
            end_counts = tuple(count + 1 for count in settled_counts)
        records = [
            start,
            sample(1, "SAMPLE", 1651, first_counts),
            sample(2, "SAMPLE", settled_elapsed, settled_counts),
            sample(3, "END", end_elapsed, end_counts),
        ]
        return spec, [bench.format_record(record, record["seq"]) for record in records]

    def test_live_parser_accepts_invalid_sample_rate_as_criteria_rejection(self):
        spec = bench.CaseSpec("slow-case", "audio", 0, 64, 144, 5, "qualify", "voices", 32, 255)
        start = {
            "v": 1, "seq": 0, "kind": "START", "case": spec.name,
            "mode": spec.mode, "filtered": spec.filtered, "channels": spec.channels,
            "voices": spec.voices, "rate": 48000, "descriptor_frames": 256,
            "duration_s": spec.duration_s, "phase": spec.phase, "axis": spec.axis,
            "low": spec.low, "high": spec.high, "dma_base": 0,
        }
        sample = {
            "v": 1, "seq": 1, "kind": "SAMPLE", "case": spec.name,
            "elapsed_ms": 5088, "frames": 128, "active": 64, "peak_active": 64,
            "render_max_us": 42337, "render_p50_us": 0, "render_p95_us": 0,
            "misses": 1, "underruns": 0, "dma_errors": 953, "warnings": 0,
            "steals": 0, "heap_internal": 12000, "heap_external": 2000000,
            "web_http_ok": 0, "web_upload_ok": 0, "web_ws_ok": 0,
        }
        end = {**sample, "seq": 2, "kind": "END", "status": "reject", "reason": "criteria"}
        lines = [bench.format_record(record, record["seq"]) for record in (start, sample, end)]
        self.assertEqual(bench.parsed_case(lines, spec)[-1]["reason"], "criteria")

        end.update(status="pass", reason="none")
        bad_lines = [bench.format_record(record, record["seq"]) for record in (start, sample, end)]
        with self.assertRaisesRegex(bench.ValidationError, "transport rate"):
            bench.parsed_case(bad_lines, spec)

    def test_expected_boot_reset_before_start_is_allowed_but_later_reset_is_not(self):
        spec = bench.CaseSpec("boot-case", "audio", 0, 64, 256, 60, "qualify", "voices", 32, 256)
        records = bench.setup_reject_records(spec, "setup")
        lines = ["rst:0x1 (POWERON_RESET)\n"] + [bench.format_record(record, record["seq"]) for record in records]
        self.assertEqual(bench.parsed_case(lines, spec)[-1]["reason"], "setup")
        with self.assertRaises(bench.ValidationError):
            bench.parsed_case(lines[:-1] + ["rst:0xc (SW_CPU_RESET)\n"] + lines[-1:], spec)
        construct = [bench.format_record(record, record["seq"]) for record in bench.setup_reject_records(spec, "construct")]
        with self.assertRaises(bench.ValidationError):
            bench.parsed_case(construct, spec)

    def test_valid_transcript_and_unknown_keys(self):
        lines = valid_transcript()
        lines[1] = lines[1].rstrip() + " future=retained\n"
        report = bench.validate_transcript(lines)
        self.assertEqual(len(report["limits"]), 4)
        self.assertEqual(report["records"][0]["future"], "retained")

    def assert_rejected(self, transform, message=None):
        with self.assertRaises(bench.ValidationError) as caught:
            bench.validate_transcript(transform(valid_transcript()))
        if message:
            self.assertIn(message, str(caught.exception))

    def test_sequence_gap_duplicate_and_reordering(self):
        self.assert_rejected(lambda lines: [line.replace("seq=2 ", "seq=3 ") if "seq=2 " in line else line for line in lines], "sequence")
        self.assert_rejected(lambda lines: lines[:3] + [lines[2]] + lines[3:], "sequence")
        self.assert_rejected(lambda lines: lines[:2] + [lines[3], lines[2]] + lines[4:], "sequence")

    def test_truncation_restart_and_panic(self):
        self.assert_rejected(lambda lines: lines[:-1], "missing final RESULT")
        self.assert_rejected(lambda lines: lines[:2] + ["Guru panic watchdog\n"] + lines[2:], "crash or reboot")

    def test_dma_counter_timing_transport_heap_and_voice_failures(self):
        replacements = (
            ("dma_errors=2", "dma_errors=3"),
            ("render_max_us=3200", "render_max_us=6401"),
            ("frames=1440000", "frames=100"),
            ("heap_internal=12000", "heap_internal=8191"),
            ("active=256", "active=255"),
            ("steals=6000", "steals=0"),
        )
        for old, new in replacements:
            with self.subTest(field=old):
                self.assert_rejected(lambda lines, old=old, new=new: [line.replace(old, new) for line in lines])

    def test_counter_regression_and_wrong_geometry(self):
        def regress(lines):
            changed = False
            output = []
            for line in lines:
                if not changed and "kind=SAMPLE" in line and "elapsed_ms=60000" in line:
                    line = line.replace("steals=6000", "steals=1")
                    changed = True
                output.append(line)
            return output
        self.assert_rejected(regress, "regressed")
        self.assert_rejected(lambda lines: [line.replace("descriptor_frames=256", "descriptor_frames=384") for line in lines], "geometry")

    def test_incorrect_next_midpoint_is_rejected(self):
        lines = valid_transcript()
        # Make the first qualification reject, then leave the following soak where a
        # midpoint qualification is required.
        lines[4] = lines[4].replace("status=pass", "status=reject").replace("reason=none", "reason=criteria").replace("render_max_us=3200", "render_max_us=7000")
        self.assert_rejected(lambda _: lines, "next bisection candidate")

    def test_missing_adjacent_reject_and_short_soak(self):
        self.assert_rejected(lambda lines: [line.replace("voice_limit=256", "voice_limit=255") for line in lines], "completed search")
        self.assert_rejected(lambda lines: [line.replace("duration_s=600", "duration_s=599") for line in lines], "duration")

    def test_result_cannot_borrow_a_soak_from_another_filter(self):
        changed = False
        output = []
        for line in valid_transcript():
            if not changed and "kind=RESULT mode=audio filtered=0" in line:
                line = line.replace("pass_case=audio-f0-pass", "pass_case=audio-f1-pass")
                changed = True
            output.append(line)
        with self.assertRaisesRegex(bench.ValidationError, "passing boundary"):
            bench.validate_transcript(output)

    def test_periodic_voice_plateau_and_heap_low_water_cannot_recover(self):
        def replace_first_sample(lines, old, new):
            changed = False
            output = []
            for line in lines:
                if not changed and "kind=SAMPLE" in line:
                    line = line.replace(old, new)
                    changed = True
                output.append(line)
            return output

        self.assert_rejected(lambda lines: replace_first_sample(lines, "active=256", "active=1"), "status disagrees")
        self.assert_rejected(lambda lines: replace_first_sample(lines, "heap_internal=12000", "heap_internal=7000"), "low-water mark recovered")

    def test_web_load_settle_accepts_initial_zeros_in_live_and_replay_validation(self):
        spec, case_lines = self.web_settle_case((0, 0, 0), (1, 1, 1))
        self.assertEqual(bench.parsed_case(case_lines, spec)[-1]["status"], "pass")

        changed = False
        output = []
        in_web = False
        for line in valid_transcript():
            if "kind=START" in line:
                in_web = "mode=web" in line
            if in_web and not changed and "kind=SAMPLE" in line:
                line = line.replace("elapsed_ms=30000", "elapsed_ms=1651").replace("frames=1440000", "frames=79248")
                for key in bench.WEB_LOAD_KEYS:
                    line = line.replace(f"{key}=30", f"{key}=0")
                changed = True
            output.append(line)
        self.assertEqual(len(bench.validate_transcript(output)["limits"]), 4)

    def test_web_load_permanent_zero_is_rejected_after_settle(self):
        spec, zero_lines = self.web_settle_case((0, 0, 0), (0, 1, 1))
        with self.assertRaisesRegex(bench.ValidationError, "status disagrees"):
            bench.parsed_case(zero_lines, spec)

    def test_web_load_single_settled_interval_without_change_is_accepted(self):
        spec, stalled_lines = self.web_settle_case((1, 1, 1), (1, 1, 1))
        self.assertEqual(bench.parsed_case(stalled_lines, spec)[-1]["status"], "pass")

    def test_web_load_stall_beyond_five_seconds_is_rejected(self):
        spec, stalled_lines = self.web_settle_case(
            (1, 1, 1), (1, 1, 1), (2, 2, 2),
            settled_elapsed=1651 + bench.WEB_LOAD_STALL_MS + 1, end_elapsed=7000,
        )
        with self.assertRaisesRegex(bench.ValidationError, "status disagrees"):
            bench.parsed_case(stalled_lines, spec)

    def test_web_load_end_accepts_the_window_boundary_and_rejects_beyond_it(self):
        spec, boundary_lines = self.web_settle_case((1, 1, 1), (2, 2, 2), (2, 2, 2), end_elapsed=10000)
        self.assertEqual(bench.parsed_case(boundary_lines, spec)[-1]["status"], "pass")
        spec, stale_lines = self.web_settle_case((1, 1, 1), (2, 2, 2), (2, 2, 2), end_elapsed=10001)
        with self.assertRaisesRegex(bench.ValidationError, "status disagrees"):
            bench.parsed_case(stale_lines, spec)

    def test_web_load_zero_or_stall_after_settle_is_rejected_by_replay_verifier(self):
        changed = False
        output = []
        in_web = False
        for line in valid_transcript():
            if "kind=START" in line:
                in_web = "mode=web" in line
            if in_web and not changed and "kind=SAMPLE" in line:
                line = line.replace("web_upload_ok=30", "web_upload_ok=0")
                changed = True
            output.append(line)
        with self.assertRaisesRegex(bench.ValidationError, "status disagrees"):
            bench.validate_transcript(output)

        changed = False
        output = []
        in_web = False
        for line in valid_transcript():
            if "kind=START" in line:
                in_web = "mode=web" in line
            if in_web and "kind=SAMPLE" in line and "elapsed_ms=60000" in line and not changed:
                for key in bench.WEB_LOAD_KEYS:
                    line = line.replace(f"{key}=60", f"{key}=30")
                changed = True
            output.append(line)
        with self.assertRaisesRegex(bench.ValidationError, "status disagrees"):
            bench.validate_transcript(output)

    def test_cli_stdout_is_bounded_and_json_is_deterministic(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            log = directory / "raw.log"
            first = directory / "first.json"
            second = directory / "second.json"
            log.write_text("".join(valid_transcript()), encoding="utf-8")
            outputs = []
            for report in (first, second):
                stream = io.StringIO()
                with contextlib.redirect_stdout(stream):
                    result = bench.main(["verify", str(log), "--json-report", str(report)])
                self.assertEqual(result, 0)
                outputs.append(stream.getvalue())
            self.assertEqual(outputs[0].count("\n"), 4)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            json.loads(first.read_text(encoding="utf-8"))


class RunnerTests(unittest.TestCase):
    def test_web_load_tracker_records_locked_outcomes_and_requires_each_class(self):
        tracker = bench.WebLoadTracker()
        threads = [threading.Thread(target=lambda: [tracker.record("http", True) for _ in range(100)]) for _ in range(4)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        tracker.record("upload", False)
        tracker.record("ws", True)
        counts = tracker.snapshot()
        self.assertEqual(counts["http"], {"success": 400, "failure": 0})
        spec = bench.CaseSpec("web-load", "web", 0, 64, 256, 60, "qualify", "voices", 32, 256)
        with self.assertRaisesRegex(bench.ValidationError, "upload"):
            bench.require_host_web_load(spec, counts, False)
        counts["upload"]["success"] = 1
        bench.require_host_web_load(spec, counts, False)

    def test_capture_opens_the_firmware_console_at_115200_baud(self):
        arguments = types.SimpleNamespace(
            artifact_dir=Path("unused"),
            serial_endpoint="rfc2217://bridge.invalid:8086?ign_set_control",
            board_ip=None,
            startup_seconds=1,
            resume=False,
        )
        runner = bench.HardwareRunner.__new__(bench.HardwareRunner)
        runner.arguments = arguments
        spec = bench.CaseSpec("serial-rate", "audio", 0, 64, 256, 1, "qualify", "voices", 32, 256)
        records = bench.setup_reject_records(spec, "setup")
        events = []

        class FakeUart:
            def __init__(self):
                self.lines = iter(bench.format_record(record, record["seq"]).encode("utf-8") for record in records)

            def __enter__(self):
                return self

            def __exit__(self, *_):
                return False

            @property
            def dtr(self):
                return None

            @dtr.setter
            def dtr(self, value):
                events.append(("dtr", value))

            @property
            def rts(self):
                return None

            @rts.setter
            def rts(self, value):
                events.append(("rts", value))

            def readline(self):
                events.append(("readline", None))
                return next(self.lines)

        uart = FakeUart()
        serial_for_url = mock.Mock(return_value=uart)
        serial_module = types.SimpleNamespace(serial_for_url=serial_for_url)

        with mock.patch.dict(sys.modules, {"serial": serial_module}), mock.patch.object(bench.time, "sleep", side_effect=lambda seconds: events.append(("sleep", seconds))):
            captured = runner.capture(spec)

        serial_for_url.assert_called_once_with(arguments.serial_endpoint, baudrate=bench.UART_BAUD, timeout=1)
        self.assertEqual(events[:6], [
            ("dtr", False),
            ("sleep", bench.RESET_PULSE_SECONDS),
            ("rts", True),
            ("sleep", bench.RESET_PULSE_SECONDS),
            ("rts", False),
            ("readline", None),
        ])
        self.assertEqual(bench.parsed_case(captured, spec)[-1]["reason"], "setup")

    def test_capture_preserves_partial_uart_on_every_live_validation_failure(self):
        arguments = types.SimpleNamespace(
            serial_endpoint="rfc2217://bridge.invalid:8086?ign_set_control",
            board_ip=None,
            startup_seconds=1,
        )
        runner = bench.HardwareRunner.__new__(bench.HardwareRunner)
        runner.arguments = arguments
        spec = bench.CaseSpec("partial", "audio", 0, 64, 256, 1, "qualify", "voices", 32, 256)
        start = bench.format_record(bench.setup_reject_records(spec, "setup")[0], 0)
        failures = {
            "panic": "Guru panic backtrace\n",
            "reboot": "rst:0xc (SW_CPU_RESET)\n",
            "malformed": "VOICE_BENCH v=1 seq=1 kind=END broken\n",
            "sequence": bench.format_record(bench.setup_reject_records(spec, "setup")[1], 2),
            "wrong-case": bench.format_record({**bench.setup_reject_records(spec, "setup")[1], "case": "other"}, 1),
        }

        class FakeUart:
            def __init__(self, text):
                self.lines = iter(line.encode("utf-8") for line in (start, text))

            def __enter__(self):
                return self

            def __exit__(self, *_):
                return False

            dtr = False
            rts = False

            def readline(self):
                return next(self.lines)

        with tempfile.TemporaryDirectory() as directory:
            for reason, failing_line in failures.items():
                with self.subTest(reason=reason):
                    raw_path = Path(directory) / f"{reason}.uart.log"
                    serial_module = types.SimpleNamespace(serial_for_url=lambda *args, **kwargs: FakeUart(failing_line))
                    with mock.patch.dict(sys.modules, {"serial": serial_module}), mock.patch.object(bench.time, "sleep"):
                        with self.assertRaises(bench.ValidationError):
                            runner.capture(spec, raw_path)
                    self.assertEqual(raw_path.read_text(encoding="utf-8"), start + failing_line)

    def test_build_failure_is_fatal_instead_of_becoming_capacity_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            arguments = types.SimpleNamespace(
                artifact_dir=directory / "artifacts",
                repository=directory,
                flash_command=directory / "flash_image.sh",
                serial_endpoint="loop://",
                board_ip="starplayer.local",
                startup_seconds=1,
                resume=False,
            )
            runner = bench.HardwareRunner(arguments)
            spec = bench.CaseSpec("compile-fails", "audio", 0, 64, 256, 1, "qualify", "voices", 32, 256)
            cached_lines = [bench.format_record(record, record["seq"]) for record in bench.setup_reject_records(spec, "setup")]
            raw_path = arguments.artifact_dir / f"{spec.name}.uart.log"
            raw_path.write_text("".join(cached_lines), encoding="utf-8")
            completed = types.SimpleNamespace(returncode=20, stdout="compiler error\n")
            with mock.patch.object(bench.subprocess, "run", return_value=completed) as run, mock.patch.object(runner, "capture") as capture:
                with self.assertRaises(bench.ValidationError):
                    runner(spec)
            run.assert_called_once()
            capture.assert_not_called()
            self.assertEqual((arguments.artifact_dir / "compile-fails.build.log").read_text(encoding="utf-8"), "compiler error\n")

    def test_candidate_build_sets_rustc_worker_stack(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            arguments = types.SimpleNamespace(
                artifact_dir=directory / "artifacts",
                repository=directory,
                flash_command=directory / "flash_image.sh",
                serial_endpoint="loop://",
                board_ip="starplayer.local",
                startup_seconds=1,
                resume=True,
            )
            runner = bench.HardwareRunner(arguments)
            spec = bench.CaseSpec("worker-stack", "audio", 0, 64, 256, 1, "qualify", "voices", 32, 256)
            lines = [bench.format_record(record, record["seq"]) for record in bench.setup_reject_records(spec, "setup")]
            completed = types.SimpleNamespace(returncode=0, stdout="build passed\n")
            with mock.patch.dict(bench.os.environ, {"RUST_MIN_STACK": "1048576"}), mock.patch.object(bench.subprocess, "run", return_value=completed) as run, mock.patch.object(runner, "capture", return_value=lines):
                runner(spec)
            self.assertEqual(run.call_args.kwargs["env"]["RUST_MIN_STACK"], "268435456")

    def test_resume_reuses_strictly_valid_uart_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            arguments = types.SimpleNamespace(
                artifact_dir=directory / "artifacts",
                repository=directory,
                flash_command=directory / "flash_image.sh",
                serial_endpoint="loop://",
                board_ip="starplayer.local",
                startup_seconds=1,
                resume=True,
            )
            runner = bench.HardwareRunner(arguments)
            spec = bench.CaseSpec("resume-valid", "web", 0, 64, 256, 1, "qualify", "voices", 32, 256)
            lines = [bench.format_record(record, record["seq"]) for record in bench.setup_reject_records(spec, "setup")]
            raw_path = arguments.artifact_dir / f"{spec.name}.uart.log"
            raw_path.write_text("".join(lines), encoding="utf-8")
            counts = {key: 0 for key in bench.HOST_WEB_LOAD_KEYS}
            load_path = arguments.artifact_dir / f"{spec.name}.web-load.json"
            load_path.write_text(json.dumps({"spec": spec._asdict(), "counts": counts}), encoding="utf-8")
            with mock.patch.object(bench.subprocess, "run") as run, mock.patch.object(runner, "capture") as capture:
                records = runner(spec)
            run.assert_not_called()
            capture.assert_not_called()
            self.assertEqual(records[-1]["reason"], "setup")
            self.assertEqual({key: records[-1][key] for key in bench.HOST_WEB_LOAD_KEYS}, counts)

    def test_resume_reuses_rejecting_one_by_one_zero_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            arguments = types.SimpleNamespace(
                artifact_dir=directory / "artifacts",
                repository=directory,
                flash_command=directory / "flash_image.sh",
                serial_endpoint="loop://",
                board_ip="starplayer.local",
                startup_seconds=1,
                resume=True,
            )
            runner = bench.HardwareRunner(arguments)
            spec = bench.CaseSpec("web-f1-q20-voices-1x1", "web", 1, 1, 1, 1, "qualify", "voices", 1, 1)
            records = simulated_case_records(spec, False)
            uart_records = [
                {key: value for key, value in record.items() if key not in bench.HOST_WEB_LOAD_KEYS}
                for record in records
            ]
            raw_path = arguments.artifact_dir / f"{spec.name}.uart.log"
            raw_path.write_text("".join(bench.format_record(record, record["seq"]) for record in uart_records), encoding="utf-8")
            counts = {key: records[-1][key] for key in bench.HOST_WEB_LOAD_KEYS}
            load_path = arguments.artifact_dir / f"{spec.name}.web-load.json"
            load_path.write_text(json.dumps({"spec": spec._asdict(), "counts": counts}), encoding="utf-8")
            with mock.patch.object(bench.subprocess, "run") as run, mock.patch.object(runner, "capture") as capture:
                resumed = runner(spec)
            run.assert_not_called()
            capture.assert_not_called()
            self.assertEqual((resumed[-1]["status"], resumed[-1]["reason"]), ("reject", "criteria"))
            self.assertEqual({key: resumed[-1][key] for key in bench.HOST_WEB_LOAD_KEYS}, counts)

    def test_resume_invalid_artifact_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            arguments = types.SimpleNamespace(
                artifact_dir=directory / "artifacts",
                repository=directory,
                flash_command=directory / "flash_image.sh",
                serial_endpoint="loop://",
                board_ip="starplayer.local",
                startup_seconds=1,
                resume=True,
            )
            runner = bench.HardwareRunner(arguments)
            spec = bench.CaseSpec("resume-mismatch", "audio", 0, 64, 256, 1, "qualify", "voices", 32, 256)
            raw_path = arguments.artifact_dir / f"{spec.name}.uart.log"
            mismatches = {
                "mode": "web", "filtered": 1, "channels": 63, "voices": 255,
                "duration_s": 2, "phase": "soak", "axis": "channels", "low": 31, "high": 255,
            }
            for field, value in mismatches.items():
                with self.subTest(field=field):
                    stale_spec = spec._replace(**{field: value})
                    stale_lines = [bench.format_record(record, record["seq"]) for record in bench.setup_reject_records(stale_spec, "setup")]
                    stale_text = "".join(stale_lines)
                    raw_path.write_text(stale_text, encoding="utf-8")
                    with mock.patch.object(bench.subprocess, "run") as run, mock.patch.object(runner, "capture") as capture:
                        with self.assertRaisesRegex(bench.ValidationError, "metadata does not match"):
                            runner(spec)
                    run.assert_not_called()
                    capture.assert_not_called()
                    self.assertEqual(raw_path.read_text(encoding="utf-8"), stale_text)

            malformed_text = "truncated UART artifact\n"
            raw_path.write_text(malformed_text, encoding="utf-8")
            with mock.patch.object(bench.subprocess, "run") as run, mock.patch.object(runner, "capture") as capture:
                with self.assertRaisesRegex(bench.ValidationError, "missing START or END"):
                    runner(spec)
            run.assert_not_called()
            capture.assert_not_called()
            self.assertEqual(raw_path.read_text(encoding="utf-8"), malformed_text)

    def test_run_requires_board_ip_before_constructing_the_hardware_runner(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            stream = io.StringIO()
            with mock.patch.object(bench, "HardwareRunner") as hardware_runner, contextlib.redirect_stdout(stream):
                result = bench.main([
                    "run", "--repository", str(directory), "--raw-log", str(directory / "raw.log"),
                    "--json-report", str(directory / "report.json"),
                ])
            self.assertEqual(result, 1)
            self.assertIn("requires --board-ip", stream.getvalue())
            hardware_runner.assert_not_called()


if __name__ == "__main__":
    unittest.main()
