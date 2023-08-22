# KV Service
Service which manages K/V stores for scripts/projects. This uses Cassandra/Scylla and uses [cqlmigrate](https://github.com/sky-uk/cqlmigrate) for data migrations. To run migrations locally, use `migrate.sh`

A one time setup is required on the keyspace to create `cqlmigrate` keyspace locks, run this command in cql shell
```cql
CREATE KEYSPACE IF NOT EXISTS cqlmigrate WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1 };
CREATE TABLE IF NOT EXISTS cqlmigrate.locks (name text PRIMARY KEY, client text);
```