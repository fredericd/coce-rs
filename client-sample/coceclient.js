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
 */
class CoceClient {
  #found = new Map();
  #notFound = new Set();

  constructor(url, providers) {
    this.url = url;
    this.providers = providers;
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

    const params = new URLSearchParams({
      id: toFetch.join(','),
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
      for (const id of toFetch) this.#notFound.delete(id);
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
