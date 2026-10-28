#!/usr/bin/env python3
"""Isolated feature/PCM proof. Run from the repository root; no golden writes."""
import os
from pathlib import Path
import subprocess
import wave

output = Path(os.environ.get("STARPLAYER_NO07_ARTIFACTS", "/tmp/no07-artifacts"))
output.mkdir(parents=True, exist_ok=True)
environment = dict(os.environ, CARGO_BUILD_JOBS="2", CARGO_INCREMENTAL="0", RUST_TEST_THREADS="2", STARPLAYER_NO07_PCM=str(output / "canonical"))
formats = "mod,s3m,mtm,xm,it,midi,smf"
for name, optional in [("off", ""), ("overlap", ",note-overlap"), ("audition", ",interactive-audition"), ("both", ",note-overlap,interactive-audition"), ("telemetry-fixed", ",note-overlap,interactive-audition,telemetry,fixed-mix"), ("telemetry-float", ",note-overlap,interactive-audition,telemetry,float-mix")]:
    arguments = ["-p", "starplayer", "--no-default-features", "--features", formats + optional]
    with (output / f"features-{name}.txt").open("w") as log:
        subprocess.run(["cargo", "tree", "--offline", *arguments, "-e", "features,no-dev"], env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
    if name == "off":
        tree = (output / "features-off.txt").read_text()
        assert 'feature "note-overlap"' not in tree and 'feature "interactive-audition"' not in tree
    with (output / f"test-{name}.log").open("w") as log:
        subprocess.run(["cargo", "test", "--offline", *arguments, "--test", "no_integration"], env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
    print(f"{name}: isolated PCM/features passed", flush=True)
for pcm in (output / "canonical").glob("*.pcm"):
    with wave.open(str(pcm.with_suffix(".wav")), "wb") as audio:
        audio.setnchannels(2); audio.setsampwidth(2); audio.setframerate(8000); audio.writeframes(pcm.read_bytes())
