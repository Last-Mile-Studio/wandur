# C# baseline used for comparison

All numbers below are from `wandur-client/docs/perf.md` (read only), measured by the owner's
harness on an Apple M3 Pro (11 cores), 18 GB, macOS 26.5.1, .NET SDK 10.0.101, Release build.
They were not re-measured for this document; see the end for when that would be needed.

## How the C# numbers were produced

From the C# repository root (`docs/perf.md`, section Commands):

```sh
dotnet build Wandur.sln -c Release
dotnet build bench/Wandur.Bench/Wandur.Bench.csproj -c Release
dotnet bench/Wandur.Bench/bin/Release/net10.0/Wandur.Bench.dll app --runs 3 --label mine
dotnet bench/Wandur.Bench/bin/Release/net10.0/Wandur.Bench.dll app startup --runs 6 --label mine-startup
dotnet bench/Wandur.Bench/bin/Release/net10.0/Wandur.Bench.dll micro --label mine
python3 bench/summarize.py .superpowers/perf/logs/app-mine.json .superpowers/perf/logs/app-mine-startup.json
```

The `app` command launches the built client once per run with `WANDUR_PERF_PROBE` set, a throwaway
`--data-dir`, `WANDUR_DIRECTORY_URL` pointing at a closed loopback port or a fake directory, and an
in-process loopback MUD (`bench/Wandur.Bench/MudServer.cs`) that streams generated ANSI text
(`AnsiGenerator.cs`: room text, channel chatter, combat lines with 16, 256 and truecolor SGR,
prompts) at a fixed byte rate in ten writes a second. The probe waits for the first rendered frame,
opens the scenario's sessions, settles 3 s, samples for 10 s (5 s for startup and directory), then
the harness reads the macOS physical footprint with `/usr/bin/footprint` and lets the app exit.

Scenarios: `startup` (no session), `session-idle` (one session, a prompt every 2 s), `flood-100k`
and `flood-100k-nochat` (one session at 100 KB/s), `flood-1m-nochat` (1 MB/s), `multi-4` and
`multi-4-nochat` (four sessions at 50 KB/s each, three inactive tabs), `multi-8-idle`,
`directory` (300 listings with 4000x3000 artwork).

Columns: Startup is process start to first usable frame. Working set is resident size; Footprint is
the macOS physical footprint (Activity Monitor's Memory), read once at the end. GC heap is after a
full collection. CPU % is of one core over the sample. Chars/s is transcript input taken in.
UI p50/p95/max is how long a job posted every 50 ms waited for the UI thread.

## Current C# numbers (after the optimizations, commit be5ebd6)

| Scenario | Runs | Startup ms | Working set MB | Footprint MB | GC heap MB | CPU % | Alloc MB/s | Chars/s | UI p50 ms | UI p95 ms | UI max ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| startup | 9 | 1149 | 202 | 226 | 26.9 | 2.7 | 0.0 | 0 | 0.1 | 0.1 | 17 |
| session-idle | 3 | 1293 | 174 | 265 | 43.7 | 6.5 | 0.1 | 14 | 0.1 | 0.1 | 30 |
| flood-100k | 3 | 1213 | 270 | 498 | 81.8 | 35.7 | 23.1 | 100437 | 0.1 | 3.9 | 19 |
| flood-100k-nochat | 3 | 1197 | 266 | 484 | 60.1 | 24.9 | 11.1 | 100594 | 0.1 | 0.1 | 5 |
| flood-1m-nochat | 3 | 1234 | 245 | 472 | 56.0 | 29.6 | 69.8 | 1001576 | 0.0 | 12.8 | 26 |
| multi-4 | 3 | 1158 | 262 | 533 | 134.0 | 42.0 | 22.6 | 200603 | 0.0 | 0.6 | 6 |
| multi-4-nochat | 3 | 1225 | 277 | 520 | 114.4 | 36.3 | 20.6 | 200485 | 0.0 | 0.2 | 9 |
| multi-8-idle | 3 | 1213 | 189 | 406 | 65.5 | 8.3 | 0.6 | 112 | 0.1 | 0.1 | 20 |
| directory | 3 | 1197 | 255 | 297 | 37.1 | 19.3 | 0.0 | 0 | 0.1 | 0.1 | 0 |

Packaged with ReadyToRun the C# app reaches a usable window in 683 ms (process start to usable
window, `docs/perf.md` changes table); the table above is the plain Release build.

## Original C# baseline (main at 69b6a73, before the optimizations)

Kept for context; the comparison uses the current numbers above.

| Scenario | Startup ms | Working set MB | Footprint MB | CPU % | Chars/s | UI p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| startup | 1077 | 211 | 226 | 2.6 | 0 | 0.1 |
| session-idle | 1176 | 176 | 261 | 7.1 | 14 | 0.1 |
| flood-100k | 1173 | 263 | 487 | 101.9 | 67070 | 3906.4 |
| flood-1m-nochat | 1128 | 244 | 474 | 36.1 | 1000952 | 17.7 |
| multi-4 | 1184 | 327 | 536 | 96.7 | 199554 | 108.4 |
| multi-8-idle | 1240 | 205 | 407 | 8.1 | 115 | 0.1 |
| directory | 1290 | 388 | 390 | 22.4 | 0 | 0.1 |

## In-process C# reference points (`micro`, after the optimizations)

| Measurement | C# result |
|---|---|
| `AnsiTerminal.Append`, 100,000 lines (8.2 M chars, 4 KB chunks) into 2,000 line scrollback | 54.9 ms, 43 MB allocated |
| `AnsiTerminal.Append` of one 4 KB chunk, full scrollback | 0.023 ms |
| `AnsiTerminal` retained, 2,000 lines | 914 KB |
| `TranscriptDisplay` (line model plus xterm) append, 100,000 lines | 101 ms |
| `TranscriptDisplay` retained, 2,000 lines | 6,026 KB (xterm buffer about 5.1 MB) |
| Directory filter, 500 worlds | 2.26 ms per query |
| Directory sort, 500 worlds | 0.57 ms per query |
| Thumbnail 4000x3000 JPEG to 800x320 | 15.8 ms, +15 MB peak RSS |

## Like-for-like plan for the Rust side

- The Rust app's opt-in probe writes the same JSON lines (`usable`, `opened`, `end`, `after-gc`,
  `done`, then waits for the `.exit` file) as `src/Wandur.Desktop/PerfProbe.cs`, and accepts and
  ignores `--data-dir`. That lets the unchanged C# harness drive it:
  `Wandur.Bench.dll app --app <rust binary> ...`, with the same loopback MUD server, the same
  scenarios, the same footprint reading and the same summarizer. The harness runs from an exported
  copy under `.superpowers/csharp-ref/` (gitignored), never from the owner's checkout.
- Where a C# column has no Rust equivalent it is mapped and labelled: GC heap becomes live heap
  bytes from a counting allocator; GC counts are reported as 0.
- Directory and artwork scenarios wait for Milestone 3.
- The C# numbers would be re-measured only if a needed scenario is missing or the Rust results look
  implausible against them; any re-measurement is recorded in `docs/measurements.md`.
