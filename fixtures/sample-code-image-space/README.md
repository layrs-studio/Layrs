# Fixture: sample-code-image-space

This fixture is a small multi-asset Layrs Space that can be read without binary
dependencies. It mixes pseudo-code, text artifacts, ASCII image-like content and
product objects.

It exists for future parser, Lens, preview/diff and Weave tests.

## Contents

- `space.layrs.txt`: human-readable Space metadata.
- `src/hello.layrs.txt`: pseudo-code text artifact.
- `artifacts/ui-frame.txt`: visual artifact represented as text.
- `layers/base.txt`: base Layer state.
- `layers/experiment-copy.txt`: experimental Layer state.
- `steps/render-preview.txt`: simulated Step output.
- `weaves/main-thread.txt`: minimal future Weave narrative.
- `notes/review.txt`: review note.

## Intended Use

- Text/code Lens preview and diff fixtures.
- Image/raw Lens fallback fixtures without large binaries.
- Weave examples that connect intent, artifacts, Steps and review notes.
