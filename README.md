# ceagle
`ceagle` checks and publishes your TI-84 Plus CE programs on [CEagle](https://ceagle.cc), from your computer or from GitHub Actions.

## Install
Download the binary for your system from the releases, rename it to `ceagle` and put it on your `PATH`, or build it with Rust:
```sh
cargo install --path .
```

## `ceagle.toml`
Put a `ceagle.toml` in the root of your project:
```toml
project = "mazerun"
files   = ["bin/MAZERUN.8xp", "bin/MAZEPK*.8xv", "bin/MAZESAVE.8xv", "README.md"]
main    = "MAZERUN"

[file."bin/MAZESAVE.8xv"]
description = "Best times and unlocked mazes"

[file."README.md"]
role = "dev"

[dependencies]
pixel-fonts = ">=1.0.0"
```

| Key | Meaning |
| --- | --- |
| `project` | The slug of your project on CEagle |
| `files` | Files of the release, in order. `*` and `?` match inside a folder, `**` matches any folders |
| `main` | Name of the variable people run. Leave it out for AppVar projects |
| `[file."path"]` | `role` (`main`, `data` or `dev`) and `description` for one file |
| `[dependencies]` | Other CEagle projects this one needs, with a minimum version |

Calculator files go to the calculator. Dev files (`.h`, `.lib`, `.txt`, `.md`) are only downloadable from the website. A calculator file can hold at most 65,504 bytes of variable data and a version can be at most 24 characters, the limits of the calculator.

## Check and publish

```sh
ceagle login
ceagle validate
ceagle publish --version 1.2.0 --notes CHANGELOG.md
```

`ceagle login` saves an API token from your CEagle account. `CEAGLE_TOKEN` works too and wins over the saved token. `ceagle validate` shows what each file was detected as and every problem to fix. `ceagle publish` uploads the release and waits until it is published:

| Result | Meaning | Exit code |
| --- | --- | --- |
| Published | Everyone can download it | 0 |
| Under review | A CEagle admin looks at it first, you get an email | 3 |
| Rejected | It was not published, the reason is shown | 4 |

Problems to fix exit with 2, other errors with 1, and a release still processing after 10 minutes with 5.

## GitHub Actions
Add your repository and workflow as a trusted publisher on your project's Publishing page. Then no secret is needed: `ceagle` gets a short-lived token from GitHub. The [`TICEagle/publish`](https://github.com/TICEagle/publish) action downloads `ceagle`, checks it and publishes.

Without the action, run `ceagle publish --version "$GITHUB_REF_NAME"` in a job with `id-token: write`.

### Build provenance
In a public repository the action also attests the release files with Sigstore and sends the attestation with the release. CEagle checks it against your files, repository, workflow, commit and run before it publishes, and the release page shows it as verified. To do the same by hand, attest the files `ceagle checksums` lists, then pass the bundle to `ceagle publish`:
```yaml
      - run: ceagle checksums > "$RUNNER_TEMP/checksums"
      - id: attest
        uses: actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8
        with:
          subject-checksums: ${{ runner.temp }}/checksums
      - run: ceagle publish --version "$GITHUB_REF_NAME" --attestation "$BUNDLE"
        env:
          BUNDLE: ${{ steps.attest.outputs.bundle-path }}
```
The job needs `id-token: write` and `attestations: write`. CEagle only takes an attestation with trusted publishing. Anyone can check a downloaded file with `gh attestation verify FILE --repo owner/repo`.

## License
`ceagle` is free software under the GNU General Public License, version 3. See `LICENSE`.
