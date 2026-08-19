[out:json][timeout:90];
area["name"="Teruel"]["admin_level"="8"]->.city;
(
  way(area.city)["highway"~"^(motorway|trunk|primary|secondary|tertiary|unclassified|residential|living_street|pedestrian)$"];
  relation(area.city)["type"="restriction"];
);
out geom;
