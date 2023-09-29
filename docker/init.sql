-- Init script for postgres database in docker
SELECT 'CREATE DATABASE actias_script_service'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'actias_script_service')\gexec

SELECT 'CREATE DATABASE actias_api'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'actias_api')\gexec

-- TODO: This is for development purposes. Do not use this in prod.
CREATE ROLE actias WITH LOGIN SUPERUSER PASSWORD 'secret';