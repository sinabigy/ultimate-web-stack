-- Benchmark fixture (TechEmpower-style "World" table: 10,000 rows). Used only by /bench/*
-- endpoints, which are disabled by default and refused in production.
CREATE TABLE bench_world (
    id            integer PRIMARY KEY,
    random_number integer NOT NULL
);
INSERT INTO bench_world (id, random_number)
SELECT g, (random() * 9999 + 1)::int FROM generate_series(1, 10000) AS g;
