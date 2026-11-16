# Movement: Apex Legends (S21 / R5) ported

`crates/shared/src/sim/` is a port of the player movement in `r5apex_ds.exe` (Season 21, the R5Flowstate
dedicated server). The source is the set of bit-exact specs in `C:\Code\apex\R5Regenis\docs\engine\*.md`,
which were read off the disassembly. A few functions those specs leave as calls were disassembled for this port
(GetAccelerate, GetMaxSpeed, UpdateSprint/CanSprint/AllowSprint, LandingSpeedScale), from
`%USERPROFILE%\Games\R5Flowstate\r5apex_ds.exe`. The two builds are the same: the offsets and constants match.

The code follows the specs' order of operations and their MIN/MAX operand order. It is not bit-exact: NaN paths,
networking notifies and the double-precision UPRIGHT tests were dropped. The player never stands on a wall
(no wallrun), so every UPRIGHT test passes.

## Run the checker

```powershell
cargo run --release -p shared --example movecheck
```

It runs 87 checks on a synthetic test world: a floor, a 22u step, a 23u step, a 60u ledge and a 100u cliff.
If the assets are present it also runs them on the real Firing Range, and it re-reads the legend's settings file
to confirm the consts below still match it. It exits with code 1 if any check fails.

Last run: 87/87 passed. A tick on the real 5.4M-triangle map costs **~50 µs** on average and ~0.3 ms at worst
(sprint, jump, slide and mantle mixed, at 60 Hz).

| Check | Got |
|---|---|
| walk / sprint / crouch-walk | 173.5 / 260.0 / 80.0 u/s |
| sprint with speed scale 1.15 (Apex's holstered numbers) | 299.0 |
| 0 to 200 u/s / 0 to 259 u/s while sprinting | 0.38 s / 0.97 s |
| sprint to a full stop | 0.22 s |
| jump takeoff vz / apex height | 271.1 (`sqrt(2*750*56)` minus three half-gravities) / 53.6 u |
| jump again right after landing (anti multi-jump) / 1 s later | 15.5 u / 53.6 u |
| bunny hop within 1 s of landing at 500 u/s | 400 (loses `skip_speed_reduce` 100, never below 260) |
| slide start from a sprint | 396 (260 + 150, capped at 400, minus the first tick of friction) |
| slide speed after 0.5 s / 1 s; the slide ends at | 283 / 189 u/s; 1.38 s |
| slide again 1.5 s later / after 2 s | no boost / boosted |
| slide jump within 0.4 s | speed minus the 140 boost; vz 257 |
| slide jump at 300..350 u/s | snaps to 350 |
| 22u step / 23u step | walks up / blocked |
| mantle onto the 60u ledge | 0.53 s; ends standing on top |
| tap-strafe 0.1 s after a jump | turns the velocity 45° (change capped at 0.7 × 260 per press) |

### Mantle exits

| Exit | Got |
|---|---|
| Finish while holding W | velocity = input × walk speed: 173.5 along +X |
| Finish while holding W, sprint latched | 260 |
| Mantle jump past the corner (boost off) | 290 along the input plus a 283.6 jump (see the note on the +30) |
| Superglide (boost off): jump-off, then crouch on the next tick | 400 horizontal (290 + slide boost, capped). Crouching one tick later gives no superglide |
| Jump while on the vertical leg with W held | ignored (spec B6.1) |
| Hop-back: jump below the corner without W | 188 away from the wall and 240 up, 0.1 s after the press |
| Cancel: let go of W, or pull back with S | the traversal ends and you drop. Past the rest fraction the mantle finishes on its own |
| Mantle boost (default): jump in the sweet spot | 338 horizontal (260 × 1.3) along the input, vz 367 (`sqrt(2*750*90)`), slide granted. Tap-strafes are blocked until you land |
| Jump outside the sweet spot (state 3) | 260 and a 56u jump (vz 289.8), no slide |

## What is ported, by spec section

| File | Functions | Spec |
|---|---|---|
| `sim/mod.rs` | PlayerMove 0x140ba8fc0 (the per-tick order), SetupMove's buttons and input vector, UpdateSprint 0x140bedf60, CanSprint 0x140bed930, AllowSprint 0x140bed5a0, GetMaxSpeed 0x140b890b0 | playermove.md §3, disassembly |
| `sim/walk.rs` | FullWalkMove 0x140b9ead0, ApplyHalfGravity 0x140b8c2a0, CheckJumpButton 0x140b8d490, WalkMove 0x140b9cdb0 (ground path), Friction 0x140b9c7c0, Accelerate 0x140b9c610, GetAccelerate 0x140b8c5d0, AirMove 0x140b9b740, AirAccelerate 0x140b9b510, CheckVelocity 0x140ba3b60 | fullwalkmove.md, walkmove.md, airmove.md, R5Regenis `native/r5r_engine/src/movement.cpp` |
| `sim/ground.rs` | CategorizePosition 0x140ba4f30 (ledge re-probe included), SetGroundEntity 0x140ba46d0 (landing energy, landing slowdown, slide-on-land), StayOnGround / GroundSnapTrace, StepSmooth / DecayStepSmooth, CheckStuck, LandingSpeedScale 0x140bf5200 | playermove.md §4-8 |
| `sim/duck.rs` | Duck 0x140ba7500 (4 states, integer-ms timer, smoothstep eye height), CanUnduck, FinishUnDuck, SetDuckViewOffset, StartSliding 0x140ba7ed0, StopSliding | playermove.md §7, slide.md |
| `sim/step.rs` | StepSlide 0x140b9ad70 (all 4 variants), TryPlayerMoveStep 0x140b9a970, StepMove 0x140bb0610 | stepslide.md |
| `sim/traversal.rs` | TraversalMove 0x140b981b0 (progress, rest, jump-off, hop-back, END and completion), TraversalPathPos, Start/Finish/EndTraversal, CheckAutoMantle (rebuilt), the mantle boost | movemodes.md part B |
| `collision.rs` | `trace_hull`: an axis-aligned box swept against the triangle BVH | (Source TraceHull semantics) |

### How it feels, briefly

- **Sprint** is a toggle-latch. One press of Shift requests a sprint for 3 s, and holding Shift keeps extending
  that by 0.1 s. Once you sprint, it stays on while you keep moving forward (within ~45°) and are not crouched.
  Stopping, walking backwards or crouching ends it. Holding Shift without forward input does nothing.
- **Acceleration is linear**, in u/s²: lowAcceleration 2500 below 120 u/s, then acceleration 450 up to walk speed,
  then sprintAcceleration 100 up to 260. There is no Source-style `accel × wishspeed`.
- **Friction** is a linear deceleration of 1250 u/s². It applies only to velocity beyond your wish: velocity along
  the input direction, up to the wish speed, is never slowed.
- **Gravity** is applied in halves at the start and end of each tick, plus a third half on a jump tick.
- **Jumping** has no buffer and no auto-hop, as in the spec: a press counts only on the tick it happens. After
  leaving the ground, the coyote grace is 0.2 s.
- **Crouch** takes 400 ms (200 ms when sliding) before the hull shrinks and crouch speed applies. Only the eye
  height eases (smoothstep). Uncrouching needs the standing hull to fit.

## Settings and cvars used

The player settings are from `pilot_survival_firesupport.json` (Bangalore, 72u tall). 20 of the 28 legend files
have the same movement values. The checker asserts the consts match the file.

| Setting | Value | Setting | Value |
|---|---|---|---|
| stand hull / view | 32×32×72 / 60 | crouch hull / view | 32×32×47 / 32 |
| speed / sprintspeed / lowSpeed | 173.5 / 260 / 120 | crouch speed | 80 |
| acceleration / lowAcceleration / sprintAcceleration | 450 / 2500 / 100 | crouch acceleration | 2500 |
| deceleration / sprintDeceleration | 1250 / 1250 | crouch decel | (-1 → 0.6 × 2500) |
| stepHeight | 22 | speedScaleSide / Back | 1 / 1 |
| airSpeed / airAcceleration / airFriction / airDrag | 60 / 500 / 0 / 0 | gravityScale | 1 |
| jumpHeight / slideJumpHeight | 56 / 50 | antiMultiJump frac / tmin / tmax | 0.3 / 0.15 / 0.75 |
| landSlowdown frac / dur / power / hmin / hmax / noSprint | 0 / 1 / 2 / 300 / 800 / 0.7 | canJumpWhileCrouched | false |
| slideRequiredStartSpeed (Air) | 200 (90) | slideSpeedBoost / Cap / Cooldown | 150 / 400 / 2 |
| slideDecel / WantToStopDecel / VelocityDecay / Accel | 100 / 400 / 0.7 / 50 | slideStop / MaxStop / MaxJump speed | 125 / 350 / 350 |
| mantleHeight | 80 | mantleDuration below / level / above / high | 0.7 / 0.6 / 0.5 / 0.35 |
| wallrunJumpOutwardSpeed / UpSpeed (hop-back) | 188 / 240 | | |

Every cvar default below was read from the exe's static ConVar objects (the default string sits at ConVar+0x50):

| Cvar | Default | Cvar | Default |
|---|---|---|---|
| sv_gravity | 750 | sv_maxvelocity | 34000 |
| AirAccelerate's air-control cvar (0x1423705d8; name not resolved) | 2.0 | sv_optimizedmovement | 1 |
| jump_graceperiod | 0.2 | skip_time / _jump_height_speed / _fraction | 1.0 / 450 / 1.0 |
| skip_speed_retain / skip_speed_reduce | -1 / 100 | jump_keyboardgraceperiodmax / min | 0.5 / 0.2 |
| jump_keyboardgrace_strength / _max | 0.7 / 0.7 | slide_max_angle_dot | 0.6 |
| slide_whileInAir / slide_auto_stand | 0 / 0 | slide_step_velocity_reduction | 10 |
| traversal_enable / traversal_cooldown | 1 / 0.5 | automantle_enable / _cooldown | 1 / .25 |
| automantle_searchdist / _mindist | 5 / 18 | automantle_height_level / above / below | 10 / 30 / -10 |
| automantle_jumpoff_duration / _anim_maxfrac | 0.1 / 0.5 | automantle_rest_frac / _below / backoff_anim_maxfrac | 0.4 / 0.3 / 0.7 |

The mantle boost and the tap-strafe gate come from R5Regenis's bridge (`engine/s21-r5sdk/src/game/server/
mantle_boost.cpp`, `tapstrafe.cpp`), not from the exe:

| Cvar | Default |
|---|---|
| mantle_boost_enabled | 1 |
| mantle_boost_disables_tap_strafes | 1 |
| mantle_boost_min_valid_traversal_frac | 0.5 |
| bridge_mantle_boost_sprint_mult | 1.3 |
| bridge_mantle_boost_jump_height | 90 |
| bridge_lurch_cutoff_speed | 1100 |
| mantle_boost_input_setting | 1 (jump) |

All of these live in `crates/shared/src/settings.rs` (`STOCK`, `CVARS`). Each `Player` carries a copy
(`settings`, `cvars`, `speed_scale`), so the server or a test can change them per player.

## Collision

Apex moves an axis-aligned box. `Collision::trace_hull(start, end, mins, maxs)` sweeps that box against every
solid triangle in the BVH. It uses an exact separating-axis test on 13 axes: the 3 box axes, the triangle normal
and 9 edge cross products.

- It returns `fraction`, `endpos`, the hit `normal`, `startsolid` and `allsolid`.
- Like Source's `DIST_EPSILON` (1/32), the box stops 1/32 short of the surface along the hit normal. A box resting
  on a floor sits 1/32 above it, and sliding along that floor does not count as a hit.
- When a triangle ties on fraction, the trace keeps the one whose normal points most upward. This is the case on
  floors built from many triangles and at ledge edges.
- **Triangles, not brushes.** The map is a triangle soup, so a box already overlapping a triangle at the start
  does not block (Quake's "startsolid, but you can get out" rule). The one exception is a move that goes deeper
  into that triangle's plane: it is blocked at fraction 0 and flagged `allsolid`, so an embedded box can walk out
  but cannot fall through a floor.
- The capsule functions (`move_capsule`, `sweep_down`, `contacts`) are still in the file. Nothing in the sim uses
  them now.

## Approximated or skipped, and why

- **TryPlayerMoveCore 0x140baeba0 and StepDownSlide 0x140baf900** have no spec yet; the docs only describe them as
  calls. They are rebuilt as Source 2013's TryPlayerMove: 4 bumps, 5 clip planes, overbounce 1. StepDownSlide is
  a slide of up to 8 iterations that stops on the first upward face. StepMove's `Q` ground-plane argument is
  ignored.
- **CheckAutoMantle 0x140b95a40** is only summarised from the decompile. Its gates are ported: the enable cvars,
  the 0.25 s cooldown, and jump held when on the ground or within 0.25 s of it. The ledge search is ours: a wall
  within `searchdist` along the view yaw, head room, room for the hull over the lip, and a walkable top no higher
  than mantleHeight. It then builds the spec's path (start below the ledge, corner, end) and
  `C = mH / (h + mH)`. One extra rule is ours too: you must be pressing forward. Hang/dangle (`pl+0x6718`) is never
  enabled.
- **TraversalMove runs on its `traversal_anim 0` path**, because there are no first-person animations to drive the
  origin. The origin follows the two-segment path at the stored progress. The animation state is still chosen
  (below/level/above/high from eye height over the ledge), because it sets the climb rate.
- **The mantle boost sweet spot.** The R5Regenis reconstruction measures `|traversal camera pitch - eye pitch| <
  1.5°` on the mantle animation, falling toward zero, with progress above 0.5. Its own note says this "opens about
  the last 0.10-0.13 s of a mantle". With no animation, the window here is the last **0.115 s** of the climb at the
  current rate, with progress above 0.5. `Player::mantle_boost.sweet_spot_frac` exposes where the window opens, for
  a future hint UI.
- **Tap-strafe (keyboard grace).** The lead's brief said the S21 dedi has no lurch code. R5Regenis's `tapstrafe.cpp`
  says otherwise: its header reads "suppression on dedi's native inline lurch". It only masks the lurch presses
  above 1100 u/s and while a full mantle boost is in the air. So AirMove's spec'd keyboard grace is ported as it
  is, with those two gates added.
- **The mantle boost makes the native jump-off unreachable.** With `mantle_boost_enabled` on (the default, as in
  S21), every jump press during a mantle latches the boost FSM. TraversalMove never sees that press, so the
  native jump-off, the hop-back and the native superglide cannot happen. They are ported and checked with the
  boost turned off (`p.cvars.mantle_boost_enabled = false`).
- **Slide super jump and the Nitro Gate launch** (`slide_super_jump.cpp`, `slide_gate_launch.cpp`) are not ported.
  The first is the `boosted_slide_jump` mod (`player_slideSuperJumpEnabled` is false in stock settings). The second
  is a map gadget.
- **Not in scope:** wallrun and its surface transitions, double jump, dodge, zipline, grapple, skydive, jetpack,
  slip triggers, launch pads, gravity fields, base velocity, moving ground, standing on players/NPCs, doors, water,
  fall damage, sounds, view punch and LedgeAssist (the jump-assist target search).
- **CheckStuck.** The 54 nudges are Source's table, which Apex fills the same way at runtime. EA skips the whole
  tick while a nudge keeps failing; here the tick still runs, because our trace lets an embedded hull walk out.
- **StartSprint / StopSprint** drive only view and animation timers (sprintStartDelay and friends), so they are
  not ported. The speed itself switches straight from walk to sprint (GetMaxSpeed), and the ramp-up is entirely
  sprintAcceleration.
- **Constants not resolved:** pl+0x6134 speed scale is 1.0 (weapon out; 1.15 reproduces Apex's holstered numbers).
  pl+0x5af0, pl+0x6678 and the time ramp pl+0x6258 are 1. The air-slow-mo time scale is 1. Surface friction is 1
  (SurfaceFromTrace with default surfaces).

## Where this disagrees with Apex-Movement-Lab (the spec wins)

- **Superglide.** The Lab's figure is 540 u/s, from the movement wiki for current Apex. Here the native superglide
  (boost off) comes out at 400: 260 (sprint) + 30 (the stale contact-plane push) at jump-off, then the slide boost,
  which is capped at 400. The S21 mantle boost gives 338 horizontal and a 90u jump.
- **The +30 contact-plane push.** CheckJumpButton pushes you off the plane you were leaning into. The contact plane
  set when the mantle began (pressing into the wall) is still set at a jump-off, so a mantle jump leaves at
  260 + 30. This is spec behaviour, not a bug in the port.
- **Slide decay.** The Lab fits the wiki's "450 → 350 in 0.21 s" with `exp(-1.2 t)`. The spec is slideDecel 100
  plus `0.7^t`, roughly 240 u/s² at 400 u/s: 396 → 283 in 0.5 s.
- **Mantle needs forward.** The Lab says Apex has no auto-mantle. The S21 binary does: in the air, a ledge is
  mantled with no jump held, as long as you push into it. On the ground you must hold jump.
- **Jump height.** The Lab uses 54.2u (285 u/s). The spec gives a jumpHeight-56 impulse, minus the gravity
  half-steps, for a 53.6u apex.
- **Step height.** The Lab uses 18; Apex's settings say 22.

## Tick rate

The client stays at a fixed 60 Hz. Apex simulates one usercmd per client frame, so the superglide (crouch exactly
one tick after the jump-off) and the mantle-boost window are measured in ticks. At 60 Hz that is a 16.7 ms window,
which is easier than at 144 fps but follows the same rule. The checker covers both.

Taps shorter than a tick still count: the client latches every key press, and each mouse-wheel notch, until the
next tick. The mouse wheel is a second jump bind (`Input::jump_pressed`).
