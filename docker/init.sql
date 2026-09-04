-- Init script for the postgres database in docker.
--
-- Runs once, on first cluster init, as the superuser created from POSTGRES_USER.
-- That role already exists by the time this runs, so this file only adds the
-- per-service databases; credentials come from the compose environment.
SELECT 'CREATE DATABASE actias_script_service'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'actias_script_service')\gexec

SELECT 'CREATE DATABASE actias_api'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'actias_api')\gexec

SELECT 'CREATE DATABASE actias_secret_service'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'actias_secret_service')\gexec
SELECT 'CREATE DATABASE actias_kv'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'actias_kv')\gexec
SELECT 'CREATE DATABASE actias_placement'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'actias_placement')\gexec
-- A second region's placement store, for the two-region compose
-- overlay (docker-compose.regions.yml); harmless on a single region.
SELECT 'CREATE DATABASE actias_placement_b'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'actias_placement_b')\gexec
