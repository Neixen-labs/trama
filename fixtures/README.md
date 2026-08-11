# Fixtures

Test and demo data. `network.geojson` and `network.trama` are the pair the byte-for-byte
equivalence test compares, so neither changes without a deliberate reason.

## `madrid.osm.json`

An OpenStreetMap extract of central Madrid, 556 ways, as Overpass writes it for `out geom;`.
Reduced to the tags the road importer reads and rounded to six decimals, which is about 11 cm —
well under the roughly 4 cm the format quantizes to anyway.

**© OpenStreetMap contributors**, licensed under the
[Open Database License](https://opendatacommons.org/licenses/odbl/) (ODbL). ODbL governs this
file and any database derived from it; it does not extend to TRAMA's own source, which stays
under the repository's licence. Anything published from this data must keep the attribution.

Regenerate it with:

```bash
curl -s -X POST https://overpass-api.de/api/interpreter --data-urlencode \
  'data=[out:json][timeout:80];way["highway"~"^(residential|primary|secondary|tertiary|unclassified|living_street)$"](40.4100,-3.7120,40.4230,-3.6950);out geom;' \
  -o madrid.osm.json
```
