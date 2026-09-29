/**
 * Usage:
 *
 *   const client = new CoceClient('http://coceserver.com:8080', 'ol,gb,aws');
 *   client.fetch(['isbn1', 'isbn2'], (id, url) => {
 *     document.querySelector(`[data-id="${id}"]`).innerHTML = `<img src="${url}">`;
 *   }).catch((err) => console.error('coce fetch failed:', err));
 *
 * fetch() returns a Promise, so a failed request can be handled with
 * .catch() (or try/catch around await) instead of failing silently.
 *
 * Coce rejects requests with more IDs than its `maxIds` setting (100 by
 * default), so ids are sent in batches of `batchSize`, in parallel. Pass a
 * third constructor argument if the server uses a lower `maxIds`.
 */
class CoceClient {
  #found = new Map();
  #notFound = new Set();

  constructor(url, providers, batchSize = 100) {
    this.url = url;
    this.providers = providers;
    this.batchSize = batchSize;
  }

  async fetch(ids, onFound) {
    const toFetch = [];

    for (const id of ids) {
      if (this.#found.has(id)) {
        onFound(id, this.#found.get(id));
      } else if (!this.#notFound.has(id)) {
        this.#notFound.add(id);
        toFetch.push(id);
      }
    }

    if (toFetch.length === 0) return;

    const batches = [];
    for (let i = 0; i < toFetch.length; i += this.batchSize) {
      batches.push(toFetch.slice(i, i + this.batchSize));
    }

    // A failed batch doesn't prevent the others from being displayed; the
    // first error is reported once they have all settled.
    const results = await Promise.allSettled(
      batches.map((batch) => this.#fetchBatch(batch, onFound)),
    );
    const failure = results.find((r) => r.status === 'rejected');
    if (failure) throw failure.reason;
  }

  async #fetchBatch(ids, onFound) {
    const params = new URLSearchParams({
      id: ids.join(','),
      provider: this.providers,
    });

    let urlPerId;
    try {
      const response = await fetch(`${this.url}/cover?${params}`);
      if (!response.ok) {
        throw new Error(`coce request failed: HTTP ${response.status}`);
      }
      urlPerId = await response.json();
    } catch (err) {
      // The request failed: don't leave these ids permanently stuck as "not
      // found", let them be retried on the next fetch() call.
      for (const id of ids) this.#notFound.delete(id);
      throw err;
    }

    for (const [id, url] of Object.entries(urlPerId)) {
      this.#notFound.delete(id);
      this.#found.set(id, url);
      onFound(id, url);
    }
  }

  reset() {
    this.#found.clear();
    this.#notFound.clear();
  }
}
