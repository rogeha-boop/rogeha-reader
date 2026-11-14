# Instructions for Claude

- **Keep the two documents in `docs/` current with every build** (asked for by the owner on 14 November 2026).
  A build is any commit that changes the program.
  - `docs/TEST-REPORT.md`: run `python tools/test_report.py` before committing. It runs the tests and rewrites
    the automated part. Then bring the part between the "by hand" markers up to date yourself: add a line for
    every scripted run, measurement or check by hand done for this build, and move things out of "Not checked"
    and "Known problems" only when they really were checked or fixed.
  - `docs/ARCHITECTURE.md`: change the section that describes whatever the build changed in how the reader is
    put together, and the version in its "Last updated" line. A fix inside one part needs only that line.
- Build and test with `--release`. Build the installer (`-p rogeha-setup`) and the Store package
  (`tools/package.ps1`) only when asked.
- When the reader is running from `target/release` or from its install folder and a build or install must
  replace the program file, rename the running file aside. Never close the owner's running reader.
- Scripted runs set `ROGEHA_DATA_DIR` to a scratch folder, so the owner's settings and recent files are
  neither shown nor changed.
