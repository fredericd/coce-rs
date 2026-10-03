/**
 * Minimal browser client for a Coce server.
 *
 * Load it with a plain script tag (it defines a global `CoceClient` class):
 *
 *   <script src="coceclient.js"></script>
 *
 * Usage:
 *
 *   const client = new CoceClient('https://coce.example.org', 'ol,gb,aws');
 *   client
 *     .fetch(['9780563533191', '2847342257'], (id, url) => {
 *       const img = document.createElement('img');
 *       img.src = url;
 *       img.alt = `Cover of ${id}`;
 *       document.querySelector(`[data-id="${id}"]`).append(img);
 *     })
 *     .catch((err) => console.error('coce fetch failed:', err));
 *
 * The client remembers what Coce answered, for the life of the page (or
 * until reset()):
 *
 * - IDs with a cover: later fetch() calls give their URL at once, without a
 *   request.
 * - IDs Coce has no cover for: never asked again.
 * - IDs being fetched: a fetch() call asking for them while the request is
 *   in progress waits for it, instead of sending a second request.
 * - IDs whose request failed (network error, HTTP error): forgotten, so a
 *   later fetch() call asks for them again.
 *
 * Coce answers 400 to requests with more IDs than its `maxIds` setting
 * (100 by default), so IDs are sent in batches of `batchSize`, in parallel.
 *
 * This client calls Coce with fetch() and reads plain JSON, which works
 * from any page since Coce allows all origins (CORS). Koha doesn't use it:
 * it has its own client, using JSONP.
 */
class CoceClient {
  /** @type {Map<string, string>} ID -> cover URL */
  #covers = new Map();
  /** @type {Set<string>} IDs Coce has no cover for */
  #noCover = new Set();
  /** @type {Map<string, Promise<Object<string, string>>>} ID -> in-flight batch */
  #inFlight = new Map();

  /**
   * @param {string} url Base URL of the Coce server, e.g.
   *   'https://coce.example.org' (a trailing slash is ignored).
   * @param {string} providers Providers to query, comma-separated, in order
   *   of preference, e.g. 'ol,gb,aws'. For each ID, Coce returns the URL of
   *   the first one that has a cover. They must be enabled on the server.
   * @param {number} [batchSize=100] Maximum number of IDs per request; keep
   *   it at most the server's `maxIds`.
   */
  constructor(url, providers, batchSize = 100) {
    this.url = url.replace(/\/+$/, '');
    this.providers = providers;
    this.batchSize = batchSize;
  }

  /**
   * Find the covers of `ids`, calling `onFound(id, url)` for each ID that
   * has one, as soon as its URL is known: immediately for URLs already
   * known, when its batch's answer arrives otherwise. IDs without a cover
   * get no call.
   *
   * @param {string[]} ids IDs to look up, usually ISBNs (any form: ISBN-10,
   *   ISBN-13, with or without hyphens). Duplicates are looked up once.
   * @param {(id: string, url: string) => void} onFound Called once per ID
   *   with a cover, with the ID as given in `ids`.
   * @returns {Promise<void>} Resolves when all IDs are settled. Rejects with
   *   the first error if a request failed; the covers of the other batches
   *   are still delivered to `onFound`.
   */
  async fetch(ids, onFound) {
    const toFetch = [];
    const waits = [];

    for (const id of new Set(ids)) {
      if (this.#covers.has(id)) {
        onFound(id, this.#covers.get(id));
      } else if (this.#noCover.has(id)) {
        // Known to have no cover: nothing to do.
      } else if (this.#inFlight.has(id)) {
        // Requested by an earlier, unfinished call: wait for its answer.
        waits.push(
          this.#inFlight.get(id).then((urlPerId) => {
            if (urlPerId[id]) onFound(id, urlPerId[id]);
          }),
        );
      } else {
        toFetch.push(id);
      }
    }

    for (let i = 0; i < toFetch.length; i += this.batchSize) {
      const batch = toFetch.slice(i, i + this.batchSize);
      const request = this.#request(batch);
      for (const id of batch) this.#inFlight.set(id, request);
      waits.push(this.#settle(batch, request, onFound));
    }

    // A failed batch doesn't prevent the others from being delivered; the
    // first error is reported once they have all settled.
    const results = await Promise.allSettled(waits);
    const failure = results.find((r) => r.status === 'rejected');
    if (failure) throw failure.reason;
  }

  /**
   * Forget everything learned so far, e.g. to look covers up again after
   * they were added at a provider. Requests in progress still complete.
   */
  reset() {
    this.#covers.clear();
    this.#noCover.clear();
  }

  /**
   * Ask Coce for one batch of IDs.
   * @returns {Promise<Object<string, string>>} ID -> URL, for IDs with a cover.
   */
  async #request(ids) {
    const params = new URLSearchParams({
      id: ids.join(','),
      provider: this.providers,
    });
    const response = await fetch(`${this.url}/cover?${params}`);
    if (!response.ok) {
      throw new Error(`coce request failed: HTTP ${response.status}`);
    }
    return response.json();
  }

  /** Record a batch's answer, and deliver its covers to `onFound`. */
  async #settle(ids, request, onFound) {
    let urlPerId;
    try {
      urlPerId = await request;
    } finally {
      for (const id of ids) this.#inFlight.delete(id);
    }
    // On failure, the IDs are neither in #covers nor in #noCover: a later
    // fetch() call asks for them again.
    for (const id of ids) {
      const url = urlPerId[id];
      if (url) {
        this.#covers.set(id, url);
        onFound(id, url);
      } else {
        this.#noCover.add(id);
      }
    }
  }
}
