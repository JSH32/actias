CREATE KEYSPACE kv_service
    WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1 };