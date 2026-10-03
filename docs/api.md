# API

| Endpoint | Purpose |
|---|---|
| `GET /cover` | Cover URLs for a list of IDs. |
| `GET /set` | Force the cover URL of an ID (protected by a token). |
| `GET /stats`, `GET /stats/daily` | Activity and state, see [administration](administration.md). |
| `GET /` | Answers `Welcome to coce`. |

## Getting covers: `/cover`

| Parameter | Description |
|---|---|
| `id` | Comma-separated IDs, usually ISBNs; at most `maxIds` (100 by default). |
| `provider` | Comma-separated providers, in order of preference, among those configured (`aws`, `gb`, `ol`, `orb`). Defaults to the configured `providers`. |
| `all` | When present, return every provider's URL instead of the first one. |
| `callback` | JSONP: wrap the result in a call to this JavaScript function. |

Without `all`, Coce returns, for each ID, the URL of the first provider that
has a cover, following the requested order:

    http://coce.server/cover?id=9780415480635,9780821417492,2847342257,9780563533191&provider=ol,gb,aws

```json
{
  "9780415480635": "https://books.google.com/books/content?id=Yc30cofv4_MC&printsec=frontcover&img=1&zoom=1",
  "9780821417492": "https://covers.openlibrary.org/b/id/12180269-M.jpg",
  "2847342257": "https://images-na.ssl-images-amazon.com/images/P/2847342257.01.MZZZZZZZZZ.jpg",
  "9780563533191": "https://covers.openlibrary.org/b/id/2520432-M.jpg"
}
```

With `all`, every provider's URL, per ID:

    http://coce.server/cover?id=9780415480635,9780821417492,2847342257,9780563533191&provider=ol,gb,aws&all

```json
{
  "9780563533191": {
    "ol": "https://covers.openlibrary.org/b/id/2520432-M.jpg",
    "gb": "https://books.google.com/books/content?id=OphMAAAACAAJ&printsec=frontcover&img=1&zoom=1",
    "aws": "https://images-na.ssl-images-amazon.com/images/P/0563533196.01.MZZZZZZZZZ.jpg"
  },
  "2847342257": {
    "aws": "https://images-na.ssl-images-amazon.com/images/P/2847342257.01.MZZZZZZZZZ.jpg"
  },
  ...
}
```

IDs without any cover are left out of the response. The order of the keys
carries no meaning.

### IDs

ISBNs can be given as ISBN-10 or ISBN-13, with or without hyphens: Coce
looks each book up once, under its ISBN-13, and caches it under that form,
so all spellings share one cache entry. Results are keyed by the IDs exactly
as requested. IDs that aren't ISBNs are passed through unchanged.

### JSONP

With a `callback` parameter, Coce returns its result as JSONP:

    http://coce.server/cover?id=9780415480635,9780563533191&provider=ol,gb,aws&callback=populateImg

```js
populateImg({"9780415480635":"https://books.google.com/books/content?id=Yc30cofv4_MC&printsec=frontcover&img=1&zoom=1","9780563533191":"https://covers.openlibrary.org/b/id/2520432-M.jpg"})
```

The callback must be a JavaScript function name (letters, digits, `_`, `$`,
`.`), otherwise Coce answers `400`. JSONP predates CORS, which Coce supports
(any origin): new clients should call Coce with `fetch()` and read plain
JSON instead. JSONP stays supported, since Koha uses it.

## Forcing a cover URL: `/set`

`/set` stores a cover URL for an ID and a provider, for 10 years, e.g. to
fix a wrong cover. It requires the `setToken` configured on the server, sent
as a bearer token (a header rather than a URL parameter, which would end up
in access logs):

```sh
curl -H "Authorization: Bearer $COCE_SET_TOKEN" \
  "http://coce.server/set?provider=ol&id=9780563533191&url=https://example.org/cover.jpg"
```

It answers `{"success": true}`. When no `setToken` is configured (the
default), `/set` is disabled: anyone could otherwise replace any cover. The
ID is normalized like in `/cover`, so a cover set for an ISBN-10 is found
when the ISBN-13 is requested.

## Errors

Errors are JSON objects, `{"error": "<message>"}`.

| Status | Endpoint | Cause |
|---|---|---|
| `400` | `/cover` | `id` missing or too short, more than `maxIds` IDs, provider not configured, invalid `callback`. |
| `400` | `/set` | Provider not configured, URL not http(s). |
| `401` | `/set` | Missing or bad token. |
| `403` | `/set` | `/set` disabled (no `setToken` configured). |
| `503` | `/set`, `/stats/daily` | Redis can't be reached. |
| `429` | any | Not from Coce: the reverse proxy's rate limit, if configured (see [deployment](deployment.md#behind-nginx)). |

A `/cover` request never fails because of a provider: an unavailable
provider just contributes no URL.

## Clients

### Koha

Koha has a built-in Coce client (`koha-tmpl/opac-tmpl/bootstrap/js/coce.js`).
It sends all the ISBNs of a result page in a single JSONP request, and has
no error handling: with more ISBNs on a page than `maxIds`, the page simply
shows no covers. Keep `maxIds` above the largest number of results per page
of the OPACs using the server.

### JavaScript sample client

[`client-sample/coceclient.js`](../client-sample/coceclient.js) is a sample
client for other web pages, used by
[`client-sample/sample-client.html`](../client-sample/sample-client.html). It
calls Coce with `fetch()`, splits large ID lists into batches of `maxIds`,
and keeps found URLs in memory:

```javascript
document.addEventListener('DOMContentLoaded', () => {
  const ids = [...document.querySelectorAll('[id^="coce-thumbnail"]')].map(
    (el) => el.dataset.id,
  );
  if (ids.length === 0) return;

  const client = new CoceClient('http://localhost:8080', 'ol,gb,aws');
  client
    .fetch(ids, (id, url) => {
      for (const el of document.querySelectorAll(`[data-id="${id}"]`)) {
        const img = document.createElement('img');
        img.src = url;
        img.title = url;
        el.append(img);
      }
    })
    .catch((err) => console.error('coce fetch failed:', err));
});
```
