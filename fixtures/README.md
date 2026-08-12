# Fixtures

Test and demo data. `network.geojson` and `network.trama` are the pair the byte-for-byte
equivalence test compares, so neither changes without a deliberate reason.

## `teruel.trama`

The whole street network of Teruel, Spain — 2,770 nodes, 3,649 edges — shipped **compiled**.
That is the point of it: 240 kB as a container against the 1.9 MB of Overpass JSON it came from,
and the playground opens it without a compile step. The example is the first pillar's claim in
the one place a visitor can weigh it.

A whole small city rather than a slice of a large one, so the questions asked of it are real:
a route across town, how far you get in ten minutes, and which streets are the only way through.
Nothing in it is hydraulic and nothing needed to be invented for it.

**© OpenStreetMap contributors**, licensed under the
[Open Database License](https://opendatacommons.org/licenses/odbl/) (ODbL). ODbL governs this
file and any database derived from it; it does not extend to TRAMA's own source, which stays
under the repository's licence. Anything published from this data must keep the attribution.

Regenerate it with `teruel.overpassql` and the compiler, both of which are deterministic:

```bash
curl -s -X POST -d @fixtures/teruel.overpassql https://overpass-api.de/api/interpreter -o teruel.osm.json
cargo run --release -p trama-cli -- compile --importer roads teruel.osm.json fixtures/teruel.trama
```

`core/trama-trace/tests/fixture.rs` checks what the published file has to be true of: one
connected network holding over 90% of the streets, crossable end to end, with critical streets
but not made only of them. The first extract this project shipped was in fragments and rendered
perfectly, which is exactly the failure a screenshot cannot show.
