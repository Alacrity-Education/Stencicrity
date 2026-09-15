# Kinematic alignment of scissor-cut stencil pieces

Design study for locating a stencil piece cut out of a stencicrity sheet, and the
PCB it belongs to, against pins on a jig with exact constraint. Numbers marked
*(est.)* are our own estimates, *(unverified)* are vendor claims we could not
cross-check. Coordinates follow the sheet convention (x right, y up, mm).

## 1. Summary and recommendation

Four round holes on four pins cannot repeat: the piece has three degrees of
freedom in the plane and the pins try to remove eight, so either the holes have
play (the piece lands anywhere inside it) or a pin interferes and the 0.12 mm
foil domes or tears. The stencil's own laser-cut features are accurate to a few
micrometres, but the scissor-cut outline is not, and neither is a hobby jig. The
fix is to give every piece three laser-cut datum *walls* and let three pins
touch them with a small, defined nesting force. The scissor cut is made to pass
*through* the datum features so the wall that survives on the piece is a laser
edge, never a scissor edge. The board is located the same way, by three pins on
its routed outline, in the same plate, so both datum systems are fixed to each
other by one machined (or printed and then calibrated) part.

Recommended scheme, in a few lines:

- Per cell, three short obround slots 4.5 x 12 mm straddling the cell edge (the
  future cut line): two on the bottom edge near its ends, one on the left edge
  at mid-height. After the cut each leaves a laser-cut wall 2.25 mm inside the
  piece's ragged edge.
- Jig: three hardened 3 mm dowel pins (ISO 8734 m6) whose cylinders touch those
  walls, plus three more 3 mm pins against the board outline in a shallow pocket
  whose floor puts the board top flush with the plate top.
- Nesting: push the piece by hand, or with a 0.3 mm spring-steel finger, toward
  the datum corner with 0.3-0.7 N total; then hold it down with two small
  magnets on the padding; print with the squeegee stroke toward the datum
  corner.
- Expected repeatability: stencil piece to pins about ±17 µm RSS (±30 worst);
  board to pins about ±60 µm RSS, dominated by the board outline, so the board
  becomes the weakest link instead of the stencil.

## 2. Why four holes on four pins does not repeat

A rigid body in a plane has 3 DOF (x, y, rotation). Blanding's rule is
`6 = C + DOF` in space, i.e. `3 = C + DOF` in the plane: exactly three
constraints locate it, every extra one is redundant and has to be absorbed by
clearance or deformation ([Blanding, *Exact Constraint*](https://books.google.com/books/about/Exact_Constraint.html?id=DtxSAAAAMAAJ),
[Hackaday review](https://hackaday.com/2019/09/11/books-you-should-read-exact-constraint-machine-design-using-kinematic-principles/)).
A round pin in a snug round hole is two constraints; four of them are eight.
Slocum's review says it directly about pinned joints: "tolerances are set so
there is always some room between components ... the accuracy and repeatability
that can be obtained is limited by the toleranced gaps"
([Kinematic couplings: a review](https://web.mit.edu/2.70/Reading%20Materials/Kinematic%20coupling%20review%20article.pdf)).
The fixture-design literature adds the two-pin case: "If the distance between
the two holes on your part differs even slightly from the distance between the
pins on your fixture, the part will bind"
([Precision alignment guide](https://mechanical-design-handbook.blogspot.com/2025/12/the-engineers-guide-to-precision.html)).

The stack for the current 5 mm holes, worst case, diametral:

| contribution | 3D-printed plate | CNC plate (ISO 2768-f) | jig-bored/reamed plate |
| --- | --- | --- | --- |
| pin position in plate, per pin | ±0.20 mm ([FDM ±0.2-0.5](https://www.hubs.com/knowledge-base/3d-printing-vs-cnc-machining/), [SLA ±0.2](https://jlc3dp.com/3d-printing/stereolithography)) | ±0.05 ([ISO 2768 f/m](https://www.rivcut.com/resources/iso-2768-tolerance-chart), [CNC ±0.05-0.1](https://www.hubs.com/knowledge-base/3d-printing-vs-cnc-machining/)) | ±0.01 *(est.)* |
| hole position on the sheet | ±0.01 (laser, see §3) | ±0.01 | ±0.01 |
| pin diameter, m6 5 mm | 5.004-5.012 ([ISO 8734 chart](https://www.ekinsun.com/custom-fasteners/dowel-pin-size-chart/)) | same | same |
| hole diameter, laser | ±0.01 *(est.)* | ±0.01 | ±0.01 |
| clearance needed so 4 pins always enter | 0.44 mm | 0.14 mm | 0.06 mm |
| resulting play = repeatability | ±0.22 mm | ±0.07 mm | ±0.03 mm |

Two things make it worse in practice. First, a hole drawn at 5.00 mm for a
5.004-5.012 mm pin is an interference fit; the foil is forced on and the hole
edge bends out of plane, which lifts the paste side off the board next to it.
Second, without a nesting force there is no preferred position inside the
play: whatever the hand does when dropping the piece decides where it ends up.
A 0.5 mm pitch pad is 0.25-0.30 mm wide, so ±0.07-0.22 mm of play is a
visible misprint.

## 3. Principles

**Three contacts, no more.** In the plane the piece needs three point
contacts: two on one straight datum (x-position and rotation) and one on a
second datum perpendicular to it. That is the 2-1 part of the 3-2-1 fixturing
rule ([RivCut 3-2-1](https://www.rivcut.com/blog/fixture-design-principles-precision));
Slocum puts practical 3-2-1 repeatability "on the order of 3-5 microns" for
hard parts ([review](https://web.mit.edu/2.70/Reading%20Materials/Kinematic%20coupling%20review%20article.pdf)).
The other exact in-plane arrangement is V-and-flat: one pin in a V (two
contacts) plus one pin on a flat (one contact). Three V's on three pins, the
in-plane picture of a Maxwell coupling, is six contacts for three DOF and
therefore over-constrained; the 3D Maxwell and Kelvin couplings are exact only
because a body in space has six DOF ([Wikipedia](https://en.wikipedia.org/wiki/Kinematic_coupling)).

**Nesting force.** Contacts only constrain while they are loaded. Hale: "The
weight of the object being supported or some other consistent nesting force
holds the surfaces in contact. A spring or compliant actuator may apply the
nesting force, but ideally it should allow all surfaces to engage freely with
minimum friction" ([Hale, Practical Exact-Constraint Design](http://pergatory.mit.edu/kinematiccouplings/documents/Theses/hale_thesis/Practical_Exact_Constraint.pdf)).
Its line of action must load all three contacts (pass through the triangle of
contacts, roughly) and operating loads (squeegee drag) should add to it, not
oppose it. For a 1.7 g foil piece gravity is useless: at 15 deg tilt the
in-plane component is 4 mN against about 5 mN of friction on the plate *(est.)*.

**Contact geometry.** A pin cylinder against a laser-cut wall on a 0.12 mm
edge is a short line contact, mechanically a point: the wall position is what
counts, the pin diameter only shifts the datum by its radius (calibrated once,
m6 spread 8 µm). A pin in a hole is two contacts fixed to a hole *diameter*
that the laser must hit to ±5 µm to avoid either play or interference. Laser
accuracy itself is not the problem: JLCPCB states "Cutting Tolerance:
±0.003mm" and a minimum aperture > 0.08 mm on 304 HTA foil in 0.10/0.12/0.15/
0.18/0.20 mm ([JLCPCB stencil capabilities](https://jlcpcb.com/capabilities/pcb-stencil-manufacturing)),
NextPCB quotes "pad position accuracy ... (±25μm)" and 3-5 deg trapezoidal walls
([NextPCB](https://www.nextpcb.com/blog/laser-stencil-in-pcb)), and stencil
lasers such as the LPKF G 60120 have "axis tolerances ... ±2 µm" over
600 x 1200 mm ([LPKF](https://www.lpkf.com/en/news-press/press-releases-teaser/lpkf-stencillaser-g-60120)).
We budget ±15 µm wall-to-aperture; the first sheet should confirm it (§7). The
3-5 deg taper shifts the contact by 0.12 mm x tan(4 deg) = 8 µm between the two
faces of the foil, which only matters if a piece is ever placed upside down.

**Abbe / lever.** A wall error e over a contact baseline B is an angle e/B
that becomes e x D/B at a pad D away ([Abbe error](https://en.wikipedia.org/wiki/Abbe_error)).
10 µm over a 33 mm baseline (two pins near the ends of a 43 mm cell) is 6 µm
at 20 mm; over a 13 mm baseline (pins at the board edge) it is 15 µm. Hence:
put the two long-datum contacts as far apart as the *cell* allows, not the
board.

**Friction and hysteresis.** Hale's estimates make repeatability proportional
to the friction coefficient, and a symmetric three-vee coupling stops centring
above µ = 0.38; friction "is a main contributor to nonrepeatability
[Slocum and Donmez, 1988]" ([Hale](http://pergatory.mit.edu/kinematiccouplings/documents/Theses/hale_thesis/Practical_Exact_Constraint.pdf)).
Clean dry steel on steel is quoted at 0.74-0.80 static
([Wikipedia](https://en.wikipedia.org/wiki/Friction)); handled parts are
nearer 0.2-0.3 *(est.)*. For our foil the friction that matters is the piece
sliding on the plate (weight 17 mN x µ = a few mN) against a 0.5 N push, so
the residual off-nest position is nanometres; the real hysteresis source is a
push that is not aimed at the corner, leaving one contact unloaded. The 2-1
fence tolerates any µ < 1 for a 45 deg push (F cos45 > µ F sin45).

**Thin-foil realities.** Euler buckling of a 43 x 42 x 0.12 mm piece pushed
in-plane over its full width is P_cr = π²EI/L² = 6.9 N (E = 193 GPa, 304
[190-203 GPa](https://www.azom.com/properties.aspx?ArticleID=965),
[Euler](https://en.wikipedia.org/wiki/Euler%27s_critical_load)); for a
130 x 110 mm piece 3.0 N, and a 10 mm wide fingertip strip buckles at
0.2-1.6 N *(est.)*. Hertz pressure of a 3 mm pin on the 0.12 mm edge is about
300 MPa at 0.5 N and 1.6 GPa at 14 N *(est.)*; annealed 304 yields at
205-310 MPa, the hard-rolled "HTA" foil considerably higher *(unverified)*. So
the nesting force must stay well under 1 N; a standard M4 ball spring plunger
(8.5-14 N, [Ganter GN 615](https://www.ganternorm.com/en/products/3.1-Indexing-locking-blocking-with-pins-and-ball-shaped-elements/Spring-plungers/GN-615-Spring-plungers-with-ball-with-slot-Steel-Stainless-Steel))
would crumple the edge. Production foils are held at 32-47 N/cm and loss of
tension gives "poor gasketing ... as well as poor alignment issues"
([Circuits Assembly](https://www.circuitsassembly.com/ca/features-itemid-fix/408-screen-printing/31697-screen-printing-1908.html));
we cannot tension a scissor-cut piece, so flatness has to come from the plate
being flush with the board top, a hold-down on the padding, and cutting with
the burr side up.

## 4. Candidate schemes

Cell geometry today (defaults): board bbox + 15 mm padding each side; two
dotted lines 2.5 mm apart centred on the cell edge; cut between them.

### a. Hole + slot on two pins (classic)

```
   +-----------------------------+      o  round hole, tight on pin A (x, y)
   |  o                     ===  |      =  slot, its long axis toward A, pin B
   |        [  board  ]          |         touches both slot walls (rotation)
   +-----------------------------+
```

Features: one round hole (pin + 0.02 mm), one slot (pin + 0.02 mm wide,
pin + 1 mm long) inside the padding, diagonal corners. Jig: two pins. Nesting:
none, the fit is the constraint. Scissor-immune (features inside the piece).
Budget: laser hole diameter ±0.01 + pin 3.002-3.008 -> 0.02-0.03 mm play ->
±0.01-0.015 mm undefined position, *if* the hole is not cut too small (then
interference, foil domes). Pros: drop-on, no push, works with the present jig
by using two of its pins. Cons: repeatability lives in a diameter the laser
must hit exactly; the foil hole edge is 0.12 mm long and wears; the two pins
must be in the plate to ±0.05 mm or the slot side jams ("the relief must be on
the pin, not the part" is the machinist's view; in 0.12 mm foil the slot is
fine because the foil, not the pin, is the cheap part). Verdict: acceptable
fallback, not kinematic in the strict sense (no defined nesting).

### b. Three-pin fence on laser walls from slots that straddle the cut line

```
 sheet, one cell (bottom-left datum corner), before cutting:

     :  cell edge = cut line (dotted pair at +-1.25)
     :
   ..:......................................  <- upper neighbour's slots
     :                                    :
     :        +---------------+           :
     :        |               |           :
   [=]        |     board     |           :   [=] x-datum slot, 4.5 x 12,
     :        |               |           :       straddling the left edge
     :        +---------------+           :
     :                                    :
   ..:...[===]....................[===]...:  <- y-datum slots straddling
     :                                        the bottom edge, centred 8 mm
                                              from the cell corners

 piece after the scissor cut (ragged edge ~), pins P on the jig:

   ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
   ~                                     ~
   ~_                                    ~
    _| wall     board                    ~
   ~ P                                   ~
   ~_|                                   ~
   ~                                     ~
   ~~~~_____~~~~~~~~~~~~~~~~~~~_____~~~~~~
        P                       P        <- pins touch the walls from outside
```

Features per cell: three obround slots 4.5 mm across the edge x 12 mm along
it, centred on the cell edge, so after a cut anywhere within ±2.25 mm of the
line (the dots are at ±1.25) both slot walls survive, one on each piece. The
wall 2.25 mm inside the piece is the datum. Jig: three 3 mm pins, centres
one pin radius (1.5 mm) from the wall, i.e. 0.75 mm *inside* the cell edge, so
the pin is tangent to the wall. Nesting: push
toward the datum corner, < 1 N. Immune to the scissor cut by construction; the
neighbouring piece gets the opposite wall for free (unused unless its datum is
on that edge). Budget: §5. Pros: true exact constraint with a defined nest,
independent of pin and hole diameters, long baseline (pins at the cell ends),
same pins and same idea as the board fence, uses the 30 mm gap that is already
there. Cons: needs a push and a hold-down; two edges of the cell are datums,
so the piece must be placed the right way up and round (mirrored bottom sides
get a mirrored jig); an operator who cuts more than 2.25 mm inside the line
destroys the wall.

Closed variant: the same slot moved fully inside the cell (walls at 2.25 and
6.75 mm), pin dropped through it and pushed to the inner wall. Cut passes
through a plain 2.25 mm web, nothing can go wrong with the cut, but the piece
must be lowered over three pins at once and each cell needs its own slots (no
sharing).

### c. V-notch + flat, keyhole / half-hole

```
   ~~~~~~\    /~~~~~~~~~~~~~~~~~~~~~_____~~~~~
          \  /                       P
           \/  <- laser V, apex 3 mm inside, opening cut away by the scissors
           P
```

A 90 deg V straddling the cut line takes x and y with one pin (two contacts),
a short slot wall at the far end of the same edge takes rotation (one
contact). Kinematic (3 contacts), scissor-immune, and it needs only *one*
datum edge, which frees the other three for packing. A single push along the
edge normal seats all three if µ < tan 45 = 1; the V centres in x with
force F sin45 against friction µF cos45. Pin diameter shifts the V datum by
1.41 x Δr (5.7 µm for the m6 spread, calibrated). Cons: the pin sits with two
45 deg contacts on a 0.12 mm edge (F/(2 cos45) each), the V walls are shorter
than a slot wall, and rotation still comes from the far flat so the Abbe gain
is zero. Two V's on two pins is four contacts, over-constrained: the same
trap as two round holes, which is why fixtures use round pin + diamond pin
([pallet practice, MIT review](https://web.mit.edu/2.70/Reading%20Materials/Kinematic%20coupling%20review%20article.pdf)).
Half-hole opening to the edge: a semicircle of pin radius is a conformal
(undefined) contact, a larger one is a single point, so it is either a bad V
or a bad flat. Keyhole (hole + neck to the edge): the pin enters through the
neck and the hole has to be tight again, scheme (a) with an entry cut.
Verdict: (c) is the runner-up; choose it when a cell can afford only one
datum edge.

### d. Other options, evaluated

- *Same pins for board and stencil*: put the slot walls exactly on the board
  outline so the three board pins also touch the stencil. Zero jig offset by
  construction, but the pins stand in the squeegee path 0-3 mm from the pads,
  the baseline shrinks to the board length, and the batch's routing offset
  cannot be corrected. Rejected for small boards; worth remembering for a
  panel with rails.
- *Compliant "star" holes* (a hole with radial slits, petals grip the pin):
  elastic averaging, zero clearance, self-nesting. In 0.12 mm foil the petals
  bend out of plane and take a set; repeatability then depends on petal
  hysteresis, guessed at 10-30 µm *(unverified)*. Elastic averaging trades
  exactness for N-averaging
  ([kinematic vs elastically averaged joints](https://www.jspe.or.jp/wp_e/wp-content/uploads/isupen/2013s/2013s-1-1.pdf));
  not worth it when a laser wall costs nothing.
- *Fold-over tab as a fence*: a hand fold in 0.12 mm stainless is neither
  straight nor at a known offset. Rejected.
- *Maxwell three-V on three pins in the plane*: over-constrained (six
  contacts), see §3. Rejected.
- *Quasi-kinematic / split-groove couplings* (Culpepper, Slocum): they replace
  dowel pins in machined assemblies ("from 5 microns using a doweled
  connection to 1.5 microns") but need machined grooves and preload through
  the parts, not applicable to a foil ([review](https://web.mit.edu/2.70/Reading%20Materials/Kinematic%20coupling%20review%20article.pdf)).

## 5. Recommended scheme in detail: (b), open straddling slots

### Stencil features per cell (defaults for gap = 30 mm, dot_line_gap = 2.5)

| item | value | why |
| --- | --- | --- |
| slot shape | obround, 4.5 mm across the cell edge, 12 mm along it, full-radius ends | straight wall 7.5 mm long, no stress-raising corners |
| slot centre line | on the cell edge (the cut line) | walls at ±2.25 mm; the cut, guided by dots at ±1.25, lands inside with 1.0 mm to spare |
| datum wall | 2.25 mm inside the cell edge, so 12.75 mm from the board at default padding | web to the board stays wide; pins stay out of the squeegee path |
| y-datum slots | two, on the bottom cell edge, centred 8 mm from the bottom-left and bottom-right cell corners | baseline = cell width - 16 mm; corner webs of 2 mm x 2.25 mm survive the cut |
| x-datum slot | one, on the left cell edge, centred at mid-height | one contact, rotation is already fixed by the y pair |
| minimum gap for the scheme | 12 mm (2 x (2.25 + 3.75)) | keeps ≥ 3.75 mm of foil between wall and board *(est.)* |
| dots | omitted where they fall inside a slot | nothing to guide there |
| datum corner | bottom-left of every cell in sheet coordinates; a mirrored bottom side therefore has its datum at the board's physical bottom-right | one rule, the report says which corner per side |

The 8 mm grid: the pins only need their *centres* on the raster. With the pin
centre tangent to the wall (0.75 mm inside the cell edge), the cell corner has to sit at
`k*8 + 0.75 mm` in x and y; along the edge the slot centres are simply put on
the raster points nearest 8 mm from the corners. This is a placement
constraint only; cells no longer have to be *grown* to a multiple of the pitch,
so the "grid waste" disappears. A universal raster plate is only worth making
if it is machined (reamed H7/G7 holes for slip-fit pins); with a 3D-printed
plate every pin is ±0.2 mm and the raster buys nothing, so print one small jig
per board instead (grid = 0, pins at the reported coordinates).

### Jig

```
 plan (top side; bottom side mirrored):        section A-A:

  +-----------------------------------+           pins 3 mm
  |                            magnet |          |‾|      board pins end 0.2 below
  |   ____________________            |          | |  ______  board top
  |  |                    |           |    ______| |_|______|__________ plate top
  | P|      board         |           |   |  pocket, board thickness - 0.1     |
  |  |____________________|   spring  |   |______________________________|
  |  P                    P   finger  |
  |P                                  |   stencil piece lies on the plate and
  |    P                       P      |   the board; pins for the stencil stand
  +-----------------------------------+   2 mm proud of the plate
```

| item | recommendation |
| --- | --- |
| plate | 6 mm aluminium, CNC (ISO 2768-f, ±0.1 on 30-120 mm) or 8 mm PETG/resin print (±0.2, then calibrate, see below); flat to 0.05 mm over the piece |
| stencil pins | 3 x ISO 8734 / DIN 6325 m6 hardened dowel 3 x 10 mm (3.002-3.008 mm, 550-650 HV30 [chart](https://www.ekinsun.com/custom-fasteners/dowel-pin-size-chart/)); pressed in (H7 in metal, print holes 2.8 and ream 3H7, or glue); 2 mm proud of the plate top; perpendicular within 0.5 deg (irrelevant for the 0.12 mm contact, matters for the board pins over 1.6 mm) |
| pin centres (per side, in board-corner coordinates from the report) | y pins: y = -(pad_y - 2.25 + 1.5) = -(pad_y - 0.75), x = 8 - pad_x and (W_cell - 8) - pad_x; x pin: x = -(pad_x - 0.75), y = H_cell/2 - pad_y |
| board pocket | board outline + 0.5 mm per side, depth = nominal thickness - 0.1 mm (JLCPCB thickness ±10 % [capabilities](https://jlcpcb.com/capabilities/pcb-capabilities)); the pocket wall does *not* locate |
| board pins | 3 x 3 mm dowels in the pocket touching the routed outline: two on the datum-side long edge at 15 % and 85 % of its length, one on the short edge at mid-length; tops 0.2 mm below the plate top so the stencil padding never rests on them |
| board with tooling holes (3.0/4.0 mm NPTH, ±0.05 [Magellan](https://magellancircuits.com/what-are-typical-sizes-and-tolerances-pcb-tooling-holes/)) | optional: round pin in one hole + a V-block/diamond pin in the other; hole position ±0.05 ([JLCPCB](https://jlcpcb.com/capabilities/pcb-capabilities)) is not better than the outline fence, so only use it when the outline is not straight (castellations, odd shapes) |
| hold-down | plate of steel, or M3 nuts/washers embedded in the print under the padding; two Ø10 x 3 N42 magnets (17.7 N hold, 3.55 N to slide [supermagnete](https://www.supermagnete.de/eng/disc-magnets-neodymium/disc-magnet-10mm-3mm_S-10-03-N)) on the far padding **after** nesting; alternative: tape hinge on the far edge |
| nesting element (optional) | 0.3 x 5 x 30 mm spring-steel strip screwed at the far corner, 1-2 mm preload, 0.25-0.5 N *(est.)*, PTFE or silicone tip; or simply two fingertips |

Relation of the two datum systems: both sets of pins are in one plate, so the
stencil-to-board offset is a fixed property of the jig. Nominally the stencil
walls are at the cell edge + 2.25 mm and the board edge at cell edge + pad, in
the same gerber coordinate system as the apertures, so the report can print the
pin pattern in board-corner coordinates directly.

Setting the offset once (per jig, and per board batch if the routing shifted):
print on a bare board, inspect the two far corners under a USB microscope or
loupe, read dx, dy and rotation. Correct by (1) swapping a *board* pin for a
pin-gauge pin of another diameter (each 0.10 mm of diameter moves that datum
0.05 mm; two different y pins give rotation), (2) a 0.06 mm Kapton wrap on a
pin, or (3) for a printed jig, re-print with the offsets. Software fallback:
a per-side `datum_offset` in `.stencicrity` shifts the slots on the *next*
sheet. Routed outline tolerance is ±0.2 mm regular / ±0.1 mm precision
([JLCPCB](https://jlcpcb.com/capabilities/pcb-capabilities)); how much of it
is batch-systematic (correctable) versus piece-to-piece is unverified, §7.

Procedure: (1) board into the pocket, push toward the datum corner until it
rests on its three pins; (2) piece on top, paste side down, walls toward the
pins; (3) push toward the datum corner, first -y (both bottom pins click),
then -x while keeping a little -y, total < 1 N, feel for the stop; (4) set the
magnets on the far padding; (5) print with the stroke *toward* the datum
corner so the drag loads the pins on both board and stencil; (6) lift the far
edge first and peel toward the pins; (7) clean; the same piece returns to the
same place without re-aligning.

### Tolerance budget, repeatability (not absolute accuracy)

| source | value (µm) | basis |
| --- | --- | --- |
| laser wall vs apertures | ±15 | vendor ±3 to ±25 (§3), to be measured |
| wall straightness over the 7.5 mm straight part | ±3 | laser axis ±2 µm class *(est.)* |
| dross / burr on the wall | ≤ 5 | "burr-free" claimed ([JLCPCB](https://jlcpcb.com/pcb-stencil)), unverified for 4.5 mm slots |
| nesting residual (one contact unloaded, friction) | ±5 | *(est.)* |
| contact deformation, thermal (17 µm/m/K x 40 mm x 5 K = 3.5) | ±3 | *(est.)* |
| **stencil piece to pins** | **±17 RSS, ±31 worst** | |
| board outline, piece-to-piece part | ±50 | of ±100-200 total, split *(unverified)* |
| routed edge roughness / glass fibre | ±30 | *(est.)* |
| pin tilt or edge not square over 1.6 mm | ±15 | 0.5 deg x 1.6 mm |
| board nesting | ±10 | *(est.)* |
| **board to pins** | **±61 RSS, ±105 worst** | |
| **stencil to board** | **≈ ±63 RSS** | dominated by the board, not the stencil |

Compared with today (±70 µm CNC / ±220 µm printed play *plus* the board), the
stencil side improves by an order of magnitude and stops being the limit.

## 6. Mapping onto stencicrity's layout code (description only)

- `LayoutParams` (`pcbstencil/model.py`): add `datum = off | holes | fence |
  fence-closed` (default `fence`; `holes` keeps today's behaviour for old
  jigs), `datum_slot_width = 4.5`, `datum_slot_length = 12.0`,
  `datum_corner_margin = 8.0`, `pin_dia = 3.0`, `pin_grid = 8.0` (0 = off;
  successor of `hole_grid`), `datum_offset` per side (dx, dy, default 0).
  `.stencicrity` migration: `holes = on/off` -> `datum = holes/off`.
- `pack()` (`pcbstencil/layout.py`): for `fence`, drop `_cell_size` growth
  (cells are exactly `board + gap` again) and keep `_snap_up` with
  `ho = datum_slot_width/2 - pin_dia/2` (0.75 mm) so cell corners land where
  the pin centres hit the raster; `_snap_down` for centring stays.
- `_holes()` becomes `_datums()`: returns per cell the three slot rectangles
  (obround, sheet coordinates) and the three pin centres, applying
  `datum_offset` to the slots only. Slots on a shared edge are merged when they
  overlap (like `_Dedupe` for holes) but two cells' slots on one edge may
  simply coexist. `Area` gets `slots` and `pins` next to `holes`.
- `_dots()`: drop dots whose centre falls inside a slot rectangle.
- Gerber writer: one obround aperture `O,4.5X12` (rotated for the x slots) or
  a region; the paste layer already carries round flashes, so this is a new
  aperture definition, nothing else. The copper reference layer should also
  get the slot outlines so the stencil house sees they are intentional.
- Preview: slots in red like other openings, pin circles as ghost outlines in
  a distinct colour, the datum corner marked.
- Report: per side, in the side's own (mirrored if needed) frame relative to
  the board's bottom-left corner: datum corner, slot rectangles (sheet and
  board coordinates), pin centres, suggested board-pin positions on the
  outline, pocket size and depth, nesting point, pin diameter; whether every
  pin centre is on `pin_grid`; and an ASCII jig sketch like the one above.
  The "cells grown for the grid" line goes away.
- Stays configurable: everything above plus `gap`, `dot_*`; the TUI Layout
  page swaps the hole rows for datum rows.

**Implemented.** stencicrity ships the *closed*-slot variant of 4b, not the
open fence: a slot has to lie completely inside its own cell, because an open
slot straddling a cell edge would cut into the neighbouring board's piece. The
slots sit on the raster of a **modular jig**: `slot_pitch` (30 mm) is the pitch
of the pin plate, every cell is rounded up to a whole number of it and its
corners sit at `k * slot_pitch - pin_offset`, so one raster covers the whole
sheet. Along the cell's bottom edge and its left edge — never the top or right
one — *every* raster position that fits is opened, so the jig may use whichever
pins suit a piece; an edge of `n` pitches carries `n - 1` slots and a cell is
grown until its longer edge has two and its shorter one has one, which is the
2 + 1 of the exact three-contact location. The piece is still pushed toward the
bottom-left datum corner until the inner walls touch their pins.

The parameters are `datum = slots | holes | none` with `slot_width` 4.5,
`slot_length` 12.0, `slot_offset` 0.5 (cell edge to outer wall), `slot_pitch`
30.0, `slot_web` 3.0 and `pin_dia` 3.0, so the inner wall is 5.0 mm inside the
edge and `pin_offset` is 3.5 mm. The 0.5 mm offset deliberately lets a slot
clip the dotted line of its *own* cell (1.25 mm in) while stopping short of the
neighbouring cell: cells touch, the two dotted lines of a shared edge stay
`dot_line_gap` apart, and the foil beside the board is left free for the
squeegee. `hole_grid` no longer applies here — it snaps the `holes` datum only.
See [layout.md](layout.md#the-datum).

## 7. Open questions, first-sheet tests

1. Ask the stencil house whether 4.5 x 12 mm openings in the paste layer pass
   their DFM (nothing under them on the copper layer); JLCPCB lists a
   *minimum* aperture only ([capabilities](https://jlcpcb.com/capabilities/pcb-stencil-manufacturing)).
2. Measure wall-to-aperture position on the first sheet (microscope with
   reticle or a flatbed scan at 2400 dpi): confirm ±15 µm and the wall
   straightness; look for dross on the slot walls.
3. Cut ten pieces and check that scissors passing through a slot do not curl
   the wall side; find the safe way round (piece side on the anvil blade,
   burr up).
4. Nest the same piece 20 times and print on a glass slide or a bare board;
   measure spread. Target ±20 µm.
5. Print 10 boards of one batch, measure paste-to-pad offset per board to
   split the outline tolerance into systematic and random parts.
6. Try the push forces: does 0.5 N ever buckle a 130 x 110 mm piece; is the
   spring finger needed or are fingertips repeatable enough.
7. Check that magnets on 0.12 mm foil do not print through as a local dome
   next to the board; check the plate-top-flush-with-board assumption at both
   ends of the ±10 % thickness range.
8. Decide 3 mm vs keeping 5 mm pins (the scheme is indifferent; 5 mm needs
   the wall 3.25 mm inside and a 6.5 mm slot).
9. Evaluate (c) V + flat if single-datum-edge cells noticeably improve packing.
10. Whether the vendor's stated foil kerf (20-40 µm, *unverified*) matters:
    it does not for walls, only the aperture compensation the house already
    applies.

## 8. Figures

Illustrative renders in `figures/` (not to scale, dimensions as in section 5):

| file | shows |
| --- | --- |
| `figures/01-sheet-with-cells.png` | a sheet with four cells, their dotted borders and the three datum slots per cell |
| `figures/02-cell-detail.png` | one cell with the scissor zone, the slots straddling the cut line and their dimensions |
| `figures/03-cut-piece.png` | the same cell after the scissor cut: ragged outline, laser-cut datum walls intact |
| `figures/04-jig-top-view.png` | the jig plate with board pocket, board pins, stencil pins, nesting direction, magnets and spring finger |
| `figures/05-jig-section.png` | section through the datum edge: plate, pocket, board flush, foil, pins, squeegee |
| `figures/06-alternatives.png` | four holes, hole + slot, V + flat and the three-pin fence side by side |
| `figures/07-procedure.png` | the seven printing steps of section 5 |
| `figures/08-tolerance-stack.png` | the repeatability budget of section 5 against today's four-hole play |

## 9. Sources

- Blanding, *Exact Constraint: Machine Design Using Kinematic Principles*, ASME 1999: https://books.google.com/books/about/Exact_Constraint.html?id=DtxSAAAAMAAJ ; review with the `6 = C + DOF` rule: https://hackaday.com/2019/09/11/books-you-should-read-exact-constraint-machine-design-using-kinematic-principles/
- Hale, *Practical Exact-Constraint Design* (thesis ch. 6; nesting force, friction estimates, centring limit µ = 0.38): http://pergatory.mit.edu/kinematiccouplings/documents/Theses/hale_thesis/Practical_Exact_Constraint.pdf
- Slocum et al., *Kinematic couplings: a review of design principles and applications* (pinned joints limited by gaps, 3-2-1 3-5 µm, Kelvin vs Maxwell, magnets as preload, pallets with round + diamond pin, QKC 5 -> 1.5 µm): https://web.mit.edu/2.70/Reading%20Materials/Kinematic%20coupling%20review%20article.pdf
- Slocum, *Design of three-groove kinematic couplings*, Precision Engineering 1992: https://www.sciencedirect.com/science/article/abs/pii/014163599290051W
- Kinematic coupling (Kelvin, Maxwell, exact constraint): https://en.wikipedia.org/wiki/Kinematic_coupling
- Kinematic vs elastically averaged joints: https://www.jspe.or.jp/wp_e/wp-content/uploads/isupen/2013s/2013s-1-1.pdf
- Exact constraint for sheet metal, diamond hole + slot, nesting force: https://www.mistywest.com/posts/make-parts-fit-right-the-first-time-with-exact-constraint-analysis/
- 3-2-1 locating: https://www.rivcut.com/blog/fixture-design-principles-precision ; round + diamond pin, binding of two round pins, H7/G7 slip fits: https://mechanical-design-handbook.blogspot.com/2025/12/the-engineers-guide-to-precision.html
- Abbe error: https://en.wikipedia.org/wiki/Abbe_error ; Euler critical load: https://en.wikipedia.org/wiki/Euler%27s_critical_load ; friction coefficients: https://en.wikipedia.org/wiki/Friction
- AISI 304 properties: https://www.azom.com/properties.aspx?ArticleID=965
- JLCPCB stencil capabilities (304 HTA, thicknesses, frameless 280x380-700x600, ±0.003 mm, >0.08 mm): https://jlcpcb.com/capabilities/pcb-stencil-manufacturing ; product page: https://jlcpcb.com/pcb-stencil
- PCBWay stencil (370x470-500x1400, 0.1-0.2 mm, framed/frameless): https://www.pcbway.com/pcb_prototype/SMT_stencil_and_Laser_Stencil.html
- NextPCB laser stencils (±25 µm position, 3-5 deg taper): https://www.nextpcb.com/blog/laser-stencil-in-pcb
- LPKF StencilLaser G 60120 (±2 µm axis): https://www.lpkf.com/en/news-press/press-releases-teaser/lpkf-stencillaser-g-60120
- Stencil tension and gasketing: https://www.circuitsassembly.com/ca/features-itemid-fix/408-screen-printing/31697-screen-printing-1908.html
- ISO 8734 / DIN 6325 m6 dowel pin chart: https://www.ekinsun.com/custom-fasteners/dowel-pin-size-chart/ ; standard text: https://cdn.standards.iteh.ai/samples/20002/ba95529de6a64437ac7454e14dbd2ef6/ISO-8734-1997.pdf
- JLCPCB PCB capabilities (outline ±0.2/±0.1, hole position ±0.05, thickness ±10 %): https://jlcpcb.com/capabilities/pcb-capabilities
- PCB tooling holes 3.0/4.0 mm ±0.05 NPTH: https://magellancircuits.com/what-are-typical-sizes-and-tolerances-pcb-tooling-holes/
- 3D printing vs CNC tolerances: https://www.hubs.com/knowledge-base/3d-printing-vs-cnc-machining/ ; JLC3DP SLA ±0.2 mm: https://jlc3dp.com/3d-printing/stereolithography ; ISO 2768 table: https://www.rivcut.com/resources/iso-2768-tolerance-chart
- Spring plunger forces GN 615: https://www.ganternorm.com/en/products/3.1-Indexing-locking-blocking-with-pins-and-ball-shaped-elements/Spring-plungers/GN-615-Spring-plungers-with-ball-with-slot-Steel-Stainless-Steel
- Magnet Ø10x3 N42 holding force: https://www.supermagnete.de/eng/disc-magnets-neodymium/disc-magnet-10mm-3mm_S-10-03-N
- Prior art, hobby jigs: adjustable hinged jig https://hackaday.com/2020/06/07/adjustable-jig-eases-pcb-stencil-alignment-process/ ; 1 mm pins through rails and stencil https://www.pcbway.com/blog/PCB_Design_Tutorial/A_very_easy_way_to_accurately_align_the_stencil_with_the_PCB_1.html
