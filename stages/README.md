# stages/

Stages as `.zip` / `.rar` archives or folders. Same caching as `chars/`.
When this folder is empty the bundled training stage is used.

## Platforms (rugen extension)

Any MUGEN stage can declare raised surfaces that players stand on. Add one
`[Platform]` section per surface to the stage `.def`, in stage coordinates
(x like `boundleft`/`boundright`, y negative = above the floor):

```
[Platform]
x = -120, -20    ; left and right edge
y = -40          ; height
```

Players land on a platform when they fall onto it from above, keep walking on
it, and fall off its edges. Characters need no changes: `Pos Y`, `PosSet` and
the landing states see the surface as the ground, so old `.cns` files work.
Draw the platform art with ordinary `[BG]` elements; F2 in a match shows the
collision lines. New characters can read `GroundY` and `OnPlatform`.

## Arenas (rugen extension)

An `[Arena]` section turns a stage into a Smash-style arena. Every key is
optional; `examples/arena/arena.def` shows a complete one:

```
[Arena]
blastzone = -520, -420, 520, 160   ; left, top, right, bottom: crossing it = ring-out
walls = 0                          ; 0 = players may leave the screen edges
stocks = 3                         ; lives per round; respawn above the start spot
floor = -240, 240                  ; the floor only exists here, pits elsewhere
spawn_y = -140                     ; respawn height
air_jumps = 1                      ; extra air jumps (overrides rugen.toml [game])
air_dash = 1                       ; double-tap forward/back in the air
fast_fall = 1                      ; hold down while falling
```

A ring-out with stocks left respawns the player with full life and a short
invulnerability; the last stock is a KO. The camera also pulls back for
height (`boundhigh` and the zoom limits still apply), so tall platform
layouts stay in view. The lifebar shows remaining stocks as marks under the
win icons.
