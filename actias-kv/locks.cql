CREATE KEYSPACE IF NOT EXISTS cqlmigrate WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1 };
CREATE TABLE IF NOT EXISTS cqlmigrate.locks (name text PRIMARY KEY, client text);