#!/bin/bash

HOSTS=${1:-localhost}
java -Dhosts=$HOSTS -Dkeyspace=kv_service -Ddirectories=migrations -DlocalDC=datacenter1 -jar cqlmigrate.jar