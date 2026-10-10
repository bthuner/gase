# Benchmarks

How fast is gase, and how do you find out what makes it slow? This folder
has the scripts, and the measurements behind the optimisations in
[`results/`](results/).

## What is measured

Five freely licensed test ROMs (fetched by `scripts/fetch-test-roms.sh`
into `target/test-roms/`; never committed):

| name | ROM | what it exercises |
|------|-----|-------------------|
| `240p-suite` | 240p Test Suite 1.23 | a menu screen; silent |
| `testpattern` | genmd-imgrom test pattern | a still full-screen picture; silent |
| `airstriker` | Airstriker (Gym Retro) | a shooter: sprites, FM music, VDP polling |
| `right2repair` | Right 2 Repair (GGJ 2020) | a game: Z80 sound driver, FM |
| `the-spiral` | The Spiral (Resistance) | a demo: heavy Z80 and PSG use |

Two numbers are reported for each:

* **Frames per second**, from `gase --headless --bench`: the speed you
  actually get. It times only the emulation (no window, no audio device,
  no frame pacing). It depends on the machine and on whatever else is
  running on it.
* **Instructions executed** (callgrind's `Ir`) for 300 frames: a count of
  the host CPU instructions. It does not depend on the machine's load, so
  it shows changes of a fraction of a percent reliably. It ignores what
  makes instructions slow (cache misses, mispredicted branches, divisions,
  SIMD doing 16 things at once), so confirm big decisions with fps too.

## Running them

```sh
scripts/fetch-test-roms.sh            # once
benchmarks/run.sh                     # fps: build, 5 rounds x 3000 frames, medians
benchmarks/callgrind.sh               # instruction counts, 300 frames per ROM
```

Both build the release binary first (respecting `$CARGO_TARGET_DIR`) unless
given binaries. To compare two versions, build each, copy the binaries
somewhere, and pass them:

```sh
benchmarks/run.sh /tmp/gase-before /tmp/gase-after   # adds a speedup column
benchmarks/callgrind.sh /tmp/gase-after
```

Knobs: `ROUNDS` and `FRAMES` for `run.sh`, `FRAMES` and `OUT` (where the
profiles go, default `target/callgrind/`) for `callgrind.sh`,
`GASE_TEST_ROMS` for another ROM directory.

`run.sh` runs every (binary, ROM) pair once per round and reports the
median over the rounds: a burst of activity elsewhere on the machine then
hits all binaries alike and the median discards it. On a shared or busy
machine use 5-9 rounds and do not trust differences below ~3%; use the
instruction counts for those.

## Profiling with callgrind

`callgrind.sh` leaves one profile per ROM in `target/callgrind/`. The
release profile keeps line tables (`debug = "line-tables-only"` in
`Cargo.toml`), so the profiles point at source lines. Then:

```sh
# Functions by their own cost ("self"): where the instructions are spent.
callgrind_annotate --inclusive=no target/callgrind/airstriker.out | head -40
# Functions including what they call: which subsystem is expensive.
callgrind_annotate --inclusive=yes target/callgrind/airstriker.out | head -60
# Who calls what, with call counts.
callgrind_annotate --tree=calling target/callgrind/airstriker.out | less
# Source lines with their cost (also inlined code, attributed to its file).
callgrind_annotate --auto=yes target/callgrind/airstriker.out | less
```

Reading them:

* Inlining moves cost around. A small function inlined into a caller is
  reported as lines of its own file *inside the caller*, e.g.
  `ym2612/operator.rs:<AudioClock>::run_until`. Lines from
  `library/core/src/...` (iterators, `min`/`max`, slice indexing) are the
  standard library inlined into your loop: count them with that loop.
* Divide by a call count to get a cost per event: instructions per pixel
  (frames x 224 lines x 320 pixels), per 68000 instruction, per audio
  sample (~888 per frame). That tells you whether a number is reasonable.
* For branch mispredictions, run
  `valgrind --tool=cachegrind --cache-sim=no --branch-sim=yes` and sort
  with `cg_annotate --sort=Bcm`.

## The rule for optimisations

Behaviour must not change: the frame hashes of the test ROMs
(`crates/core/tests/test_roms.rs`), the WAV output and the CPU test vectors
must stay bit-identical. And readability comes first, so each fast path
keeps a **readable reference** next to it, named in its documentation,
with a unit test proving the two agree (usually on thousands of random
inputs). See "Performance" in [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md#9-performance).

## Results

[`results/`](results/) holds dated measurements: the machine, the exact
commands, fps medians, instruction counts and the most expensive functions
before and after each round of work. Raw callgrind profiles are not
committed (they are large and easy to regenerate).
