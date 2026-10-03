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
