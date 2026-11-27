# Bundled Vector Typography Assets (`sbm-ui`)

These TrueType (`.ttf`) vector font files are bundled into `sbm-ui` and loaded
into Iced so SBM Core shares the exact same Atkinson Hyperlegible Next and
Atkinson Hyperlegible Mono typography system as `sbm-uefi`.

## Upstream Sources & Provenance

| Font Family | Upstream Repository | Pinned Commit | License | Reserved Font Names |
| --- | --- | --- | --- | --- |
| Atkinson Hyperlegible Next | <https://github.com/googlefonts/atkinson-hyperlegible-next> | `7925f50f649b3813257faf2f4c0b381011f434f1` (2025-02-21) | SIL OFL 1.1 ([OFL-AtkinsonHyperlegibleNext.txt](OFL-AtkinsonHyperlegibleNext.txt)) | None declared |
| Atkinson Hyperlegible Mono | <https://github.com/googlefonts/atkinson-hyperlegible-next-mono> | `154d50362016cc3e873eb21d242cd0772384c8f9` (2024-11-20) | SIL OFL 1.1 ([OFL-AtkinsonHyperlegibleMono.txt](OFL-AtkinsonHyperlegibleMono.txt)) | None declared |

## Bundled Files

Generated reproducibly from upstream static TTFs using
`sbm-uefi/scripts/optimize-fonts.py --profile latin-full` (`fontTools`):

| File | Bundled Size | Codepoints | Role Usage |
| --- | ---: | ---: | --- |
| `AtkinsonHyperlegibleNext-Regular.ttf` | 37,560 B | 362 | `Body`, `Caption` |
| `AtkinsonHyperlegibleNext-SemiBold.ttf` | 38,256 B | 362 | `Display`, `Heading` |
| `AtkinsonHyperlegibleMono-Regular.ttf` | 26,020 B | 359 | `Technical` |
| **Total** | **101,836 B** | — | All 5 roles |

## License Compliance

Both font families are distributed under the SIL Open Font License, Version 1.1.
Neither upstream `OFL.txt` declares a Reserved Font Name. All copies remain
strictly under SIL OFL 1.1 with copyright and license notices preserved in both
the binary `name` tables and the companion `OFL-*.txt` files.
