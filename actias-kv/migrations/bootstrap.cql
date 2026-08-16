-- NetworkTopologyStrategy from the start: SimpleStrategy cannot be changed to
-- it later without a full repair, and it is the only strategy that works once
-- a second datacenter exists. replication_factor applies to every DC.
CREATE KEYSPACE IF NOT EXISTS kv_service
    WITH replication = {'class': 'NetworkTopologyStrategy', 'replication_factor': 1};
