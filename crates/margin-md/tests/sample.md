---
title: Field notes on the lighthouse rewrite
tags: [beacon, rust, notes]
---

# Lighthouse Rewrite

The **beacon controller** turns the lamp on at dusk and off at dawn. This page
collects *design notes*, open questions and a few ***very important*** caveats.
Text can be ~~deleted~~, `inline code` sits in the line, and escapes like \*this\*
stay literal. Entities: &copy; 2026, 3 &lt; 4 &amp; 5 &gt; 2, caf&#233;.

A hard break ends this line  
and the next one starts here.\
Backslash breaks work too.

## Links and references

Read the [rotation spec](https://example.org/spec "Rotation spec") first, then the
[maintenance guide][guide] and the [glossary]. Bare URLs such as https://example.org/notes
and autolinks like <https://example.org/a?b=c> are clickable. Jump to the [table section](#tables).
A footnote reference sits here[^lamp].

[guide]: https://example.org/guide "Guide"
[glossary]: /docs/glossary.md

![Lamp schematic](images/lamp.svg)

### Lists

- Lens cleaning
- Bulb replacement
  - Halogen
  - LED
    - Warm white
- Clockwork check

1. Climb the stairs
2. Open the hatch
3. Wind the mechanism

7) Start counting at seven
8) Then eight

- [x] Order spare bulbs
- [ ] Repaint the railing
- [ ] Log the **fog horn** test

Loose list:

- First paragraph of the item.

  A second paragraph inside the same item.

- Another item with code:

  ```sh
  beacon --check --verbose
  ```

> Block quotes hold other blocks.
>
> - including lists
> - and *formatting*
>
> > Nested quotes work as well.

> [!NOTE]
> The lamp must never run during maintenance.

> [!WARNING]
> Do not look directly into the lens.

> [!TIP]
> Wind the clock before the storm season.

#### Code

```rust
/// Turns the lamp on when the light level drops.
fn update(level: f32, lamp: &mut Lamp) -> bool {
    if level < THRESHOLD && !lamp.on {
        lamp.on = true; // "dusk"
        return true;
    }
    false
}
```

    indented code block
    second line

~~~json
{ "interval_ms": 1500, "colour": "white", "enabled": true }
~~~

## Tables

| Part      | Interval | Cost (€) | Notes                       |
|:----------|:--------:|---------:|-----------------------------|
| Lens      | 6 months |    120.0 | Clean with `alcohol`        |
| Bulb      | 2 years  |     45.5 | Keep **two** spares         |
| Clockwork | 1 year   |    300.0 | Pipe \| escaped in a cell   |

Setext heading
--------------

Another one
===========

***

<div class="banner">
  Raw HTML stays <b>literal</b>.
</div>

Inline tags like <kbd>Ctrl</kbd> show as text, a <br> breaks the line, and <!-- comments --> vanish.

##### Small heading

###### Smallest heading

[^lamp]: The lamp is a 1000 W halogen unit with a backup LED array.
