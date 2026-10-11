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
collision lines.
