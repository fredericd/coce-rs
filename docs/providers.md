# Providers

| Code | Provider | Lookup | Account |
|---|---|---|---|
| `aws` | Amazon | one request per ISBN | none |
| `gb` | Google Books | one request per batch | none |
| `ol` | Open Library | one request per batch (sometimes a few) | none |
| `orb` | [ORB](https://www.base-orb.fr) | one request per batch | user and API key |

A request lists the providers in order of preference; Coce queries them in
parallel. For each ID, the client gets the URL of the first provider in that
order that has a cover, or every provider's URL with `all` (see
[API](api.md)). List several providers, so that one's gaps are filled by
the others.

## Amazon: `aws`

Amazon has no public cover lookup API usable without an affiliate account.
Coce probes the predictable image URL instead, with a `HEAD` request per
ISBN: `https://images-na.ssl-images-amazon.com/images/P/<ISBN-10>.01.MZZZZZZZZZ.jpg`.

* A `200` or a `403` (hotlink protection on some valid images) means the
  image exists. Missing covers don't return a `404`: Amazon serves a 1x1
  GIF placeholder with a `200`, which Coce recognizes (real covers are
  JPEG).
* The URL is keyed by ISBN-10. A 979-prefixed ISBN-13 has no ISBN-10
  equivalent, and its ASIN can't be derived from it, so Amazon is never
  queried for these ISBNs: it would return the cover of an unrelated book.
  979-prefixed ISBNs are increasingly common, as the 978 range runs out
  (France uses 979-10, the US 979-8): always list other providers after
  `aws`, e.g. `aws,gb,ol`, so they can supply those covers.
* ISBNs are probed one after the other, 30 ms apart, since Amazon blocks
  bursts. A large uncached batch can therefore take longer than the global
  `timeout`: Amazon stops at that point and the ISBNs probed so far are
  cached, so that the next request for the same IDs carries on.
* Images have a fixed size.

## Google Books: `gb`

Coce queries Google Books' dynamic links API
(`https://books.google.com/books?bibkeys=...&jscmd=viewapi`) for the whole
batch in one request, and returns the thumbnail URL, bumped to medium size
(`zoom=1`).

* Google Books silently ignores the IDs beyond the 100th of a request. With
  the default `maxIds` (100), requests never exceed it.

## Open Library: `ol`

Coce looks ISBNs up in batch through the [Open Library Search
API](https://openlibrary.org/dev/docs/api/search), and returns the cover of
the edition matching each ISBN, as a [Covers
API](https://openlibrary.org/dev/docs/api/covers) URL by cover ID
(`https://covers.openlibrary.org/b/id/<id>-M.jpg`), which, unlike access by
ISBN, is not rate limited. `imageSize` selects `small`, `medium` (default)
or `large` (`-S`, `-M`, `-L`).

* The cover taken is the edition's, not the work's: a work's cover may be
  that of any other edition (an audiobook, a large print edition...).
* The search returns at most one matching edition per work. When several
  requested ISBNs belong to the same work, the ones left out are queried
  again, up to three rounds.
* Known limitation: when several edition records share the same ISBN, the
  search returns only one of them, so a cover attached to another
  duplicate record is missed.

## ORB: `orb`

Coce queries the ORB API (`https://api.base-orb.fr/v1/products?eans=...`)
for the whole batch in one request, with the configured `user` and `key`
(HTTP basic authentication), and returns the front cover of each EAN.

* `imageSize` selects the `thumbnail` (default, 160 px high) or the
  `original` (500 px high), falling back to the thumbnail when ORB has no
  original.
* ORB is the provider best covering French publishers, including
  979-10-prefixed ISBNs that Amazon can't serve.
* A batch of 100 EANs is answered in full, without pagination.
* Wrong credentials make every call fail with `401`: the provider is then
  disabled by its circuit breaker, and a warning logged.

## Failures and circuit breaker

Coce tells apart a provider answering "no cover for this ID", which is
cached like a found cover, from a provider failing to answer: network error,
timeout (`providerTimeout`), throttling (`429`), server error (`5xx`),
unexpected response. A failure is never cached, so the ID is looked up again
once the provider is back, instead of being reported as coverless for the
whole cache duration.

Each provider has its own circuit breaker. After 3 consecutive failed calls,
the provider is disabled for `providerRetry` seconds: Coce stops calling it
and answers with the other requested providers, without waiting. When the
delay is over, a single request tests the provider again: on success it is
re-enabled, otherwise it stays disabled for another `providerRetry` seconds.
A `400 Bad Request` doesn't count as a failure, since it can be caused by
the IDs sent rather than by the provider.

The breaker state is kept in memory, per Coce instance, and is reset on
restart. `/stats` shows it (see [administration](administration.md#stats)),
and the logs record each provider disabled or re-enabled.
