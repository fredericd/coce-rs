# Performance

A `/cover` request takes one of two very different paths:

* **IDs in cache**: one Redis read per requested provider, no outside call.
  Coce answers in about a millisecond and serves a high request rate.
* **IDs not in cache**: Coce queries the providers, so the response time is
  theirs, from a few hundred milliseconds to several seconds, bounded by
  `timeout`. This is the cost the cache saves on every later request for the
  same IDs.

Example, measured on an Apple M5 laptop (10 cores), with Coce built in
release mode and Redis 8 running on the same machine, for a request of 4
ISBNs on 2 providers:

| Scenario | Result |
|---|---|
| first request, cache empty | 414 ms |
| same request, cache warm | 1.4 ms |
| `ab -k -n 300000 -c 50`, cache warm (3 runs of about 3 s) | 97,500 to 98,800 requests/s, 99% under 1 ms |
| same without keep-alive (`ab -n 10000 -c 50`) | 38,900 requests/s, 99% under 3 ms |

These figures only illustrate the order of magnitude: they depend on the
machine, and here the load generator, Redis and Coce shared the same
processor (Coce used about 5 of the 10 cores). Measure on your own server,
ideally through your reverse proxy.

To reproduce:

```sh
cargo build --release
./target/release/coce &

URL='http://127.0.0.1:8080/cover?id=9780415480635,9780821417492,2847342257,9780563533191&provider=gb,aws'
curl -s "$URL"                  # fill the cache first
ab -k -n 300000 -c 50 "$URL"    # cached path
```

## Comparison with Coce for Node.js

Coce was first written in Node.js ([fredericd/coce](https://github.com/fredericd/coce)).
Both versions were measured side by side with their own Docker images, as
shipped, on 2026-10-07: coce-rs at `45cd508`, Coce Node 2.0.0 at `d370fc5`.

| | coce-rs | Coce Node | Ratio |
|---|---|---|---|
| Image size (on disk / compressed) | **8 MB** / 2.8 MB | 255 MB / 64 MB | ×32 |
| Memory, idle | **11 MB** | 60 MB | ×5.5 |
| Memory, after load | **38–48 MB** | 96–155 MB | ×2.5–3 |
| First request, cache empty (median of 10 ISBNs) | **0.24 s** | 0.43 s (max 4.3 s) | ×1.8 |
| Cached requests, one at a time | **10,600 req/s** (0.09 ms) | 6,300 req/s (0.16 ms) | ×1.7 |
| Cached requests, peak throughput | **115,000–128,000 req/s** | 8,300 req/s | ×14–15 |
| Cores used | **all** | one | |
| Docker log volume for ~400,000 requests | **4 KB** | 1.1 GB | |

Throughput by number of concurrent requests (cached `/cover` of 3 ISBNs with
`&all`, median of 3 interleaved runs, 4 cores):

| Concurrency | coce-rs | Coce Node | Coce Node, `LOG_LEVEL=warn` |
|---|---|---|---|
| 1 | 10,636 req/s | 6,275 | 8,483 |
| 10 | 63,107 | 8,321 | 11,253 |
| 50 | 104,691 | 8,254 | 11,341 |
| 100 | 114,652 | 8,296 | 11,181 |
| 200 | 117,038 | 8,340 | 10,948 |

What explains the gap:

* **One core against all of them.** The Node image runs a single Node
  process, which tops out at one core (111% CPU under load) whatever the
  machine offers: going from 4 to 6 cores changed nothing for it. coce-rs
  spreads requests over every core (426% CPU at 100 concurrent requests).
  Node can scale with PM2 cluster mode (`ecosystem.config.js`), but its
  Docker image does not use it.
* **Logging.** At its default `info` level, Coce Node writes about 2.7 KB of
  log per request: it costs about 25% of its throughput and, under Docker's
  default `json-file` driver with no rotation, fills the disk (1.1 GB after
  this benchmark). coce-rs logs nothing per request by default (see
  [logging](logging.md)).
* **Image contents.** The Node image carries Alpine, the Node runtime and
  `node_modules`; coce-rs ships a single static binary on `scratch` (see
  [deployment](deployment.md#image-design)).

With uncached IDs, both mostly wait for the providers, so the gap there is
small and noisy; coce-rs was faster on 8 ISBNs out of 10.

### Conditions and caveats

* Apple M5 laptop, Docker in a Colima VM (aarch64, 4 CPUs and 4 GB, then 6
  CPUs for the scaling check). Each server had its own Redis 7 container,
  with providers `aws,gb,ol`.
* `ab` ran in a container on the same Docker network, sharing the VM's cores.
  At 6 cores coce-rs, `ab` and Redis together used about 5.5 cores and `ab`
  (single-threaded) neared its own limit, so coce-rs's peak is understated.
* Absolute figures depend on the machine. On a small 4-core mini PC, over the
  LAN, coce-rs served about 2,000–2,400 req/s, a figure bounded by the
  network and the load generator more than by Coce.
