# Web UI manual QA checklist

Run this before a release and after changes to the UI. It needs a browser and a throw-away data directory; nothing here talks to a service outside your machine.

Setup:

```sh
mkdir /tmp/vigil-qa && cd /tmp/vigil-qa
python3 -m http.server 8000 &                      # serves page.html below
printf '<h1>Shop</h1><p class="price">$10</p><p class="ad">buy now</p>' > page.html
VIGIL_DATA_DIR=./data VIGIL_LISTEN=127.0.0.1:5800 VIGIL_BASE_URL=http://127.0.0.1:5800 \
  VIGIL_ADMIN_PASSWORD=qa-password-1 vigil serve
```

A webhook catcher for the notification (any HTTP server that prints requests will do):

```sh
python3 -c "
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get('content-length', 0)))
        print(body.decode(), flush=True); self.send_response(200); self.end_headers()
http.server.HTTPServer(('127.0.0.1', 8001), H).serve_forever()"
```

## Sign in

- [ ] `http://127.0.0.1:5800/` redirects to the login page; the page has no menu.
- [ ] A wrong password says so; the right one (`qa-password-1`) opens the watch list.
- [ ] Opening `/tags` while signed out, signing in, lands on `/tags`.
- [ ] *Sign out* in the menu returns to the login page; Back does not show the list.

## Add a watch, filter and preview

- [ ] *Add watch*. With the address empty the preview says to enter one.
- [ ] Address `http://127.0.0.1:8000/page.html`: the preview shows the whole text (heading, price, ad) with status 200.
- [ ] *Add filter*, kind *Keep or remove by CSS selector*, selector `.price`: the preview shows only `$10`. No page reload; typing in the selector updates the preview after a short pause.
- [ ] Break the selector (`div[[`): the filter turns red with a message and the preview lists the problem. Fix it: the preview comes back.
- [ ] Add two more filters, move one up and one down, remove one: what was typed in the others stays.
- [ ] Console (browser developer tools) shows no errors and no Content-Security-Policy violations.

## Notification target

- [ ] *Notifications*, *Add target*: name `catcher`, address `json://127.0.0.1:8001/hook`. The list shows it with its address, a plain-HTTP warning, and `***` where the path is secret.
- [ ] *Send test*: the row says the message was sent, and the catcher printed it.
- [ ] *Edit*: the address field is empty (placeholder shows the hidden address); saving with it empty keeps it.
- [ ] *Turn off*, *Turn on*; *Delete* asks first and *Cancel* puts the row back.

## Save, check, notification, diff

- [ ] Back on the add form (name `Widget`, `.price` filter), tick `catcher` under *Send to*, *Add watch*. The list shows it; within a few seconds its status turns *OK*.
- [ ] Change the page: `printf '<h1>Shop</h1><p class="price">$12</p><p class="ad">buy now</p>' > page.html`. Press *Check now*: the row updates, and the catcher prints a message whose `diff_url` / link points at `/watch/<id>/diff?from=...&to=...`.
- [ ] Open that link: the page shows `$10` removed and `$12` added. Switch to *Side by side* and back; the *From* and *To* pickers work.
- [ ] Change only the ad text and press *Check now*: no new notification, no new version (the filter hides it).
- [ ] The watch's page lists the checks and the notification; *History* lists two versions; each can be opened.
- [ ] Open the diff link while signed out (private window): the login page appears, and signing in lands on the diff.

## Other pages

- [ ] *Tags*: add a tag with a colour, a default interval and `catcher` as default target; put it on a watch from the edit form; the list can be filtered by it from the tag chip.
- [ ] *Settings*: switch to Ukrainian; every page is in Ukrainian and the choice survives a reload. Change the password (wrong current password is refused); the other browser is signed out. *Sign out everywhere* ends this session too.
- [ ] *Pause* a watch: its row is dimmed and it is not checked; *Resume*.
- [ ] *Delete* a watch: asks first; it disappears from the list.

## Looks

- [ ] Dark mode (OS setting): every page is readable; diffs, badges and tags keep their meaning.
- [ ] Phone width (about 390 px): no horizontal scroll on the list, the form or a diff; buttons are usable; the diff scrolls inside its box.
- [ ] Keyboard only: *Skip to content* works, every button and link can be reached, focus is visible.
- [ ] Zoom to 200%: still usable.
