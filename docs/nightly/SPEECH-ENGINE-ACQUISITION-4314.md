# Verified native speech engine acquisition — nightly 4314

The Linux x64 GNU speech package graph is pinned to the actual nightly 4314
`pnpm-lock.yaml` at 83a3674ffd84ffb33de768210c806cb1ac53fd80:

| Package | Version |
| --- | --- |
| `transcribe-cpp` | 0.4.1 |
| `koffi` | 3.3.6 |
| `@koromix/koffi-linux-x64` | 3.3.6 |
| `@transcribe-cpp/linux-x64-cpu-vulkan` | 0.4.1 |

`runtime/speech/engine-packages-4314.json` retains all four exact SHA512 SRI pins,
public archive URLs, unpacked sizes/counts and the source lock/catalog SHA256.
Other platforms' optional packages are not fetched. This graph contains 140
regular files totaling 70,933,783 bytes. All package contents and licenses remain
in the installed graph. The source license copies and hashes are also retained
under `docs/licenses/speech-engine-4314/`.

## Acquisition and independent verification

`tools/acquire-speech-engine` runs the acquisition implementation only inside
private rootless Podman. Both acquisition and runtime entrypoints first inspect
local Podman security and require `Rootless=true`; `--userns=keep-id` alone is
not treated as proof. No npm lifecycle/install/build script runs. Each archive
is hashed against the source pin before extraction. The extractor accepts only
regular files inside the package prefix, with bounded aggregate size and exact
file count; links, devices, duplicate names and traversal are refused. Every
output file is created exclusively in a new owned directory. Failed directories
are retained for inspection, never recursively deleted or reused.

An optional model must be selected by exact ID from the complete pinned native
catalog, `runtime/speech/catalog-4314.json`. No model is downloaded by default.
Model URLs contain the exact source revision; byte length and SHA256 must both
match the catalog. Original public metadata/model cards are retained alongside
receipts. The model weights, archives and native libraries are not committed to
this repository.

For an existing acquisition, `--cache` reuses explicit read-only input artifacts
but still verifies their bytes. `--offline` additionally disables networking.
`tools/verify-speech-engine` independently rechecks each original archive SRI,
each archive member against the installation receipt, every installed file and
the selected model against the original catalog. It rejects extra files rather
than accepting a same-size sentinel or an incomplete dependency directory.

Example private acquisition:

```sh
python3 tools/acquire-speech-engine \
  --out /absolute/new/private-install \
  --model-id handy-computer/moonshine-tiny-gguf
```

The output is an explicit trusted application-owned installation input for the
Rust `EnginePackage`/`SealedFile` admission APIs. There is no arbitrary MCP URL
or package-path setting and no automatic live profile installation.

## Real CPU acceptance

`tools/test-real-speech-engine` compiles and snapshots the acceptance executable,
its runtime libraries and an admitted Node 26.13.0 under the shared compile lock,
then runs with read-only installation/binaries, private writable HOME/tmp, no
network, no host devices/display and a bounded process/memory budget. An explicit,
normalized absolute `CARGO_TARGET_DIR` unique to the worktree is required; the
runner never falls back to the shared server target. Missing targets and rootful
engines fail before artifact creation or Node admission.

The first real catalog model is **Moonshine Tiny Q8**, the smallest source entry:
35,466,912 bytes, revision `f5c11906eba3f44cf305eed30feb9cbfb0b4b9d0`,
SHA256 `2fd348d7b38f97d309cc3ec6848f3f57f537b80244950f07d2637e463f95a3a1`.
The catalog and pinned model card declare MIT, inherited from Useful Sensors'
base model. Neither pinned model repository supplies a standalone license file;
the original model card/declaration is retained, without inventing a copyright
holder or substituting a different license. Package license files are preserved
verbatim.

Actual Rust→admitted Node→unchanged native factory→`transcribe-cpp 0.4.1` completed
CPU model load, buffered transcription and native stream fallback finalization
using one second of synthetic silence. The engine's raw backend is **`CPU`**;
its finish result is a transcript **string**. Both remain unchanged, matching the
whole native producer. Initial fixture assumptions about lowercase casing and a
`{text}` finish object failed and were corrected only in the fixture. The two
original failing receipts are retained.

The final physical checks pass in both server feature modes; the first complete
run took approximately four seconds and ended with confirmed graceful cleanup.
This proves the selected real CPU/model path, not acoustic accuracy, microphone
capture, GPU execution, all catalog models, full-max streaming memory, other OS
packages, or HTTP/application activation. No capabilities are enabled here.
ModelStore selection/download/cancellation/removal/unload consumers remain a
separate implementation checkpoint.
