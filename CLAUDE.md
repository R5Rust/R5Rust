# r5rust

From-scratch Rust/wgpu Firing Range that should look and play 1:1 like **retail Apex Legends** (current season).
Start with `HANDOFF.md` (state + next task) and `README.md` (how to run). Retail-vs-Flowstate rule:
`C:\Code\CLAUDE.md` "Apex: retail is the reference".

## Rules

- **Reference = R5Flowstate S30** (R5F 2.0, `%USERPROFILE%\Games\R5Flowstate`): its scripts + in-game captures decide
  how things look and behave (Kainz, 2026-11-20). Assets stay extracted from retail; no re-extraction.
- **Never use S21** (binary, scripts, modkit, S21-derived notes) without Kainz's explicit OK (2026-11-21).
  One OK so far (2026-11-22): the RUI host (`tools/rui`, Flowstate S21 ui.dll + ui.rpak) for drawing HUD/UI assets
  only (the S30 install has no separate ui.dll). Mechanics still come from S30.
- **Ghidra = S30**: project `C:\Code\reverse-engineering\reverse-engineering-projects\ghidra\R5F_S30.gpr`
  (`r5apex_dx12_s30.exe`, md5 9fcca108...). `R5Apex.gpr` is the old S21 exe: never use it. Check `get_program_info` first.
  Older docs (apex-lighting.md, skybox.md, highlight.md) cite S21 addresses; re-verify anything you rely on in S30.
- No game assets in any repo; never push this repo (old commits hold extracted models). See HANDOFF.md.
- Video settings: no shading options (no specular/bloom/exposure/fog/decal toggles). Lighting always renders
  like Apex; settings only trade detail and resolution (textures, models, anisotropy, shadows, AA, scale).
