# Editing existing images and figures

In **Edit a PDF → Edit text and images**, click a figure to select it. Drag the selected
figure to move it, drag a corner to resize it proportionally, or press Delete to remove
that placement. The context menu also rotates or flips it. Changes support undo and
survive saving/reopening. For single-figure transforms, a clipping path exclusive to one image moves and resizes
together with its pixels, including nested graphics states, intervening state operators
and paths split across content streams. A clip shared with other content stays where it is.
When the figure already sits alone in a `q … cm … Q` (its own placement, or the one an
earlier edit added), moving or resizing it again changes that `cm`, so repeated edits don't
nest one more graphics state each time. Moving a privately cropped figure again likewise
changes the `cm` before its clip instead of adding another.

PDF figures are not always bitmap images. Imported diagrams often use a **Form XObject**,
which packages text, vector paths, and nested images into one object. PdfCraft selects
this object using its `BBox` and optional `Matrix` and edits it as a whole. Its original
content, resources, clipping box, and transparency settings remain intact. Moving or
deleting one use of a shared figure does not alter its other placements or pages.

**Replace Image** and **Save Image As** apply only to raster Image XObjects; these menu
items are disabled for grouped Form artwork. **Ungroup** (right-click) draws one placement's
content in place so its text and pictures can be edited; while grouped, the tool does not edit
a Form's individual internal elements. It does not select arbitrary ungrouped vector paths or
edit inline images (`BI`/`ID`/`EI`). A Form's bounding box may include whitespace.

**Edit with** (right-click → Edit with) hands a raster figure to a program outside PdfCraft —
Paint, GIMP, Photoshop, one you pick with **Other program…**, or the system's own *Open with*
dialog — and draws the saved picture in place of the one that left. The picture is exported as a
PNG into a private folder under the system's temporary directory, never beside the document:
a document may sit in a folder Drive or OneDrive syncs, where a file an editor rewrites is
uploaded, locked or restored mid-edit. **Edit at** chooses the resolution the editor is handed —
the figure's own pixels by default, or 150, 300 or 600 dpi, which follows `/UserUnit` and a
turned placement. The program is never waited for and never killed, because a single-instance
editor hands the file to the copy already open and exits at once, and stays open long after the
first save; the saved file's stamp is what is watched, and it must hold still for two polls
before it is read, so a program writing in place is not read half-written. A save becomes one
undoable page-image edit, and `/Intent`, `/Interpolate` and `/OC` are carried over from the
replaced object, so a picture that belonged to a layer stays on it. The placement is found by
its image object and its box, not by its position in the page's list, so an edit that moved the
index meanwhile (deleting another image, arranging, ungrouping, undo) does not misplace it. A
placement that was deleted, or one drawn twice in the same box, is not put back: the message
says why and the file is left alone. A picture saved while its tab is in the background is
applied when that tab comes to the front. The document on disk is untouched until you save.

For automation, `page_images` lists both kinds with `kind: "image"` or `kind: "form"`.
Forms have `pixels: [0, 0]`, since they have no single bitmap resolution. Use the returned
number with `image_edit` (move, rotate, flip_horizontal, flip_vertical, delete).
Coordinates are top-left-origin page points. `image_save` and `image_edit`'s replace action
return a clear error for Form artwork.

Selection frames follow the visible portion of artwork after page and path clipping. Clicking
editable text takes precedence over overlapping artwork, including labels inside an image's
visible rectangle. Hidden image placements have no selection frame. Bezier extrema determine
curved-path frames. Hit testing evaluates the actual clipping paths with their nonzero or
even-odd fill rule, so holes and empty corners do not intercept clicks. Intersections of several
curved clips can still have a loose bounding frame. Dragging the visible frame moves or resizes
the full placement by the same change; resizing keeps the opposite corner fixed, including
horizontal-only or vertical-only corner drags, and grabbing a grip slightly off-centre does not
jump. A selected corner keeps control of its drag even over a text label; the drag previews one
frame and applies one undoable edit on release. Exclusive crops and their holes move
with the selected image, also when the picture sits inside a clip it shares with its section
(a report drawing each picture as `q re W n cm Do Q` within a table's clip): the picture's own
clip scales with it, and shared clips stay fixed without moving neighbouring text or artwork.

`image_hit` applies the same path and hole checks headlessly. `page_images` retains `rect` as
the full placement rectangle used by `image_edit` and adds `visible_rect` for selection (null
when fully clipped). `object_list` reports visible artwork bounds while keeping the original
source indexes. A selection frame does not create, remove or enlarge the PDF's clipping path.
