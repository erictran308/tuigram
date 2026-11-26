# Upstream bug report: kitty photos stay blank until a resize or a second open

Ready to file at `erictran308/tuigram` (issue, or a PR with the fix below).
Fixed in this fork by commit `ce7b055`
("Send kitty images before the frame that draws their placeholders").

## Title

Kitty: a photo opened in the viewer stays blank ("Loading…") until a resize or a second open

## Body

**Environment:** kitty (Unicode placeholders, `U=1`), tuigram 1.8.1, any OS.

**Steps:**

1. Open a chat with a photo.
2. Press `Enter` on it to open the photo viewer.
3. The viewer frame comes up, the title says `loading…` (or nothing at all
   once the file is downloaded), and the photo never appears.
4. Resize the terminal window — the photo appears at once. Alternatively
   close the viewer with `Esc` and press `Enter` again — it shows
   immediately on the second open.

**Expected:** the photo shows as soon as it is decoded, on the first open.

**Root cause.** With kitty's protocol, tuigram draws the image's Unicode
placeholders in the frame and only *after* the frame sends the escape code
that transmits the image itself (`Images::send_pictures` runs after
`terminal.draw`). But the kitty graphics protocol requires the opposite
order — the image must be created first:

> So an image is first created using the normal graphics protocol escape
> codes (albeit in quiet mode (`q=2`)) … Then, the actual image is
> displayed by getting the host application to emit normal text consisting
> of `U+10EEEE` and various diacritics …
> — <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>

Kitty draws a placeholder cell naming an image id it doesn't have yet as
blank, and does not re-render the cell when the image arrives afterwards.
So the placeholder cells flushed in the frame stay blank forever, even
though the image itself reaches the terminal a millisecond later.

This explains both workarounds:

- a resize redraws every cell — the placeholders are re-emitted, and the
  image is already known to the terminal;
- a second open works because the image was transmitted during the first
  open and is still in the terminal under the same id.

**Fix.** Transmit newly encoded kitty images *before* the frame that first
draws their placeholders. In this fork: `Images::add_ready`/`add_avatar`
queue the key of every newly encoded kitty image, and
`Images::prime_transmits` (called next to `send_deletions`, ahead of the
frame) renders each queued image into a scratch `Buffer`, which makes
ratatui-image produce its transmit code exactly once
(`KittyProtoState::make_transmit`), extracts the code from the first
placeholder cell with the existing `transmit_in`, and sends it to the
terminal. The real frame then draws pure placeholders naming an image the
terminal already has. See commit `ce7b055`, including the test
`a_kitty_image_is_sent_before_the_frame_drawing_its_placeholders`.
