#!/bin/bash

HOSTS=${CQL_HOSTS:-localhost}

echo "Creating locks (if not exist)"
cqlsh -f locks.cql $HOSTS
echo "Running migrations"
java -Dhosts=$HOSTS -Dkeyspace=kv_service -Ddirectories=migrations -DlocalDC=datacenter1 -jar cqlmigrate.jar