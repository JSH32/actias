-- The placement keyspace, replicated inside this region's datacenter
-- alone: placement is DC-local by design, unlike kv's keyspace, which
-- every datacenter carries. The datacenter name and factor are filled
-- in from the service's configuration when this runs.
--
-- Tablets are off because the claim is a lightweight transaction, and
-- scylla does not serve those on a tablet-based keyspace; vnodes do.
CREATE KEYSPACE IF NOT EXISTS placement
    WITH replication = {'class': 'NetworkTopologyStrategy', '{{dc}}': {{rf}}}
    AND tablets = {'enabled': false};
