<!-- SPDX-License-Identifier: MPL-2.0 -->

# plcc device catalog

One TOML manifest per device: the board, what its runtime exposes, and how to
build, flash and talk to it. The format is `docs/device-manifest.md` in the
plcc repository.

| File | Device |
|---|---|
| `arduino-opta.toml` | Arduino Opta with the plcc generic runtime (`plcc-arduino`) |
| `simulator.toml` | plcc studio's in-browser simulator |

Rules for this directory:

- Manifests and this README only. Tools read the `.toml` files; nothing else
  here is imported.
- Bump `device.version` on every change to a manifest. Projects keep their own
  copy and are offered the update; they are never updated silently.
- Values come from the runtime and the vendor's board support package, with
  the source in a comment, not from guesses.
- A manifest is untrusted data to every tool that reads it: `[flash]` can only
  narrow the flasher's built-in limits for a USB id, never widen them.
