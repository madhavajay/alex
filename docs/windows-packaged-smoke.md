# Windows 11 packaged smoke

Run this manual gate on a disposable, clean Windows 11 x86-64 VM before marking
the Windows V1 packaged-smoke checklist item complete. The runner downloads the
specified GitHub release through `install-release.ps1`, verifies the archive
checksum, and exercises the installed binaries and real per-user Task Scheduler
entry.

From a checkout of the same release commit, open a normal (non-Administrator)
PowerShell window and run:

```powershell
$CandidateVersion = Read-Host "Candidate version (for example 0.1.29-beta.14)"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File `
  .\packaging\ci-windows\smoke-installed.ps1 `
  -Version $CandidateVersion `
  -EvidencePath "$PWD\windows-smoke-evidence.json"
```

Replace the example version with the candidate. The VM user must start with no
`AlexDaemon` scheduled task, no process on `127.0.0.1:4100`, no
`%LOCALAPPDATA%\Alex\bin`, no `%USERPROFILE%\.alex`, and no
`ALEX_HOME` override. The script refuses to overwrite any of these.

The smoke uses a PowerShell/.NET loopback mock as an OpenAI-compatible Exo
endpoint. It does not require provider credentials and does not contact a real
provider. Public network access is used only by the release installer to fetch
the GitHub ZIP and checksum.

It verifies:

- both packaged executables and their reported version;
- the `AlexDaemon` action is the installed `alex.exe daemon`, runs as a
  per-user Task Scheduler entry, and reaches `/health`;
- `alex web --no-open`, `/ui/`, its static assets, and loopback-only `/connect`;
- model publication and one deterministic request through loopback Exo;
- trace metadata and the persisted response body;
- `alex service restart` replaces the Task Scheduler engine PID;
- the same trace and exact response body remain readable after restart; and
- task, process, state directory, installed binaries, and the user `PATH`
  modification are cleaned up.

The evidence file is JSON and deliberately excludes the temporary local key.
It remains after cleanup and must contain `"passed": true`, distinct positive
`pid_before` and `pid_after` values, and a trace ID. Preserve that file with the
candidate/version and VM details. A script or CI parse pass is preparation, not
a Windows packaged-smoke pass; do not update the checklist until this command
has actually succeeded on the Windows 11 VM.

Use `-KeepArtifacts` only while debugging. It retains temporary mock logs, but
still removes Alex, its scheduled task, daemon state, and the PATH change.
