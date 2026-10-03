# JavaScript client sample

* [`coceclient.js`](coceclient.js): a small browser client for Coce, to
  display covers on any web page. It defines a global `CoceClient` class;
  its usage is documented at the top of the file.
* [`sample-client.html`](sample-client.html): a page using it to display
  the covers of a few ISBNs.

Koha doesn't need it: Koha has its own Coce client (see
[API](../docs/api.md#koha)).

## Running the sample

1. Start Coce on `localhost:8080`, e.g. from the repository root:

   ```sh
   cargo run --release
   ```

   (with a Redis server on `localhost:6379`), or with Docker, see the
   [quick start](../README.md#quick-start).

2. Open `sample-client.html` in a browser, straight from the file system.

The page asks Coce for its ISBNs and displays each cover as soon as its URL
arrives, or "no cover". If Coce isn't reachable, the page says so. To use
another Coce server or other providers, change `COCE_URL` and `PROVIDERS`
in the page's script.

It works from a local file, or from any web site, because Coce allows
requests from all origins (CORS).

## Using the client in your pages

```html
<script src="coceclient.js"></script>
<script>
  const client = new CoceClient('https://coce.example.org', 'ol,gb,aws');
  client
    .fetch(['9780563533191', '2847342257'], (id, url) => {
      const img = document.createElement('img');
      img.src = url;
      img.alt = `Cover of ${id}`;
      document.querySelector(`[data-id="${id}"]`).append(img);
    })
    .catch((err) => console.error('coce fetch failed:', err));
</script>
```

* `fetch(ids, onFound)` calls `onFound(id, url)` for each ID with a cover,
  as soon as its URL is known, and returns a promise settled once all IDs
  are: it rejects if a request failed, while the covers of the other
  requests are still delivered.
* The client remembers the answers for the life of the page: known covers
  are given back at once, IDs without a cover aren't asked again, and IDs
  already being fetched aren't requested twice. `reset()` forgets them.
* Long ID lists are split into requests of at most 100 IDs, Coce's default
  `maxIds`; pass a third argument to the constructor if the server uses a
  lower limit.
* Build images with `createElement` and `src`, as above, rather than
  inserting the URL into HTML (`innerHTML`).
