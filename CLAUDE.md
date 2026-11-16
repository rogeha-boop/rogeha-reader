# Instructions for Claude

- **Every request goes through a pull request** (asked for by the owner on 16 November 2026), so each
  change is tracked and approved on GitHub. Work on a new branch, commit there, push the branch and open a
  pull request against `main` with `gh pr create`. Never push to `main` directly, and do not merge a pull
  request unless the owner says so. One request, one pull request; a follow-up to the same request goes
  onto the same branch while its pull request is open.
- **Keep the two documents in `docs/` current with every build** (asked for by the owner on 14 November 2026).
  A build is any commit that changes the program.
  - `docs/TEST-REPORT.md`: run `python tools/test_report.py` before committing. It runs the tests and rewrites
    the automated part. Then bring the part between the "by hand" markers up to date yourself: add a line for
    every scripted run, measurement or check by hand done for this build, and move things out of "Not checked"
    and "Known problems" only when they really were checked or fixed.
  - `docs/ARCHITECTURE.md`: change the section that describes whatever the build changed in how the reader is
    put together, and the version in its "Last updated" line. A fix inside one part needs only that line.
- **For every Store package** (`tools/package.ps1`; asked for on 16 November 2026), before building it:
  - update `docs/ARCHITECTURE.md` and `docs/TEST-REPORT.md` for that version as described above, even when
    the program did not change since the last build (then the "Last updated" line and a new test run);
  - update `package/store-listing.md`: the Releases table (version, commit, status), "What's new in this
    version", and the description, product features and checks wherever the new version changes them.

  After building it, send the Rogeha Studio Website session (session id
  `local_51083b7e-75c3-4a27-b2bc-e261e3460b3d`, project `C:\Users\rogeh\StudioProjects\rogehastudio-website`)
  a message with the new version number and what is new in it for users, so it can update the reader's page
  on rogehastudio.com. Say that a version is in the Store only once it has passed certification.
- Build and test with `--release`. Build the installer (`-p rogeha-setup`) and the Store package
  (`tools/package.ps1`) only when asked.
- When the reader is running from `target/release` or from its install folder and a build or install must
  replace the program file, rename the running file aside. Never close the owner's running reader.
- Scripted runs set `ROGEHA_DATA_DIR` to a scratch folder, so the owner's settings and recent files are
  neither shown nor changed.
