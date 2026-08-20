export default () => ({
  port: parseInt(process.env.PORT, 10),
  externalServices: {
    scriptServiceUri: process.env.SCRIPT_SERVICE_URL,
    kvServiceUri: process.env.KV_SERVICE_URL,
  },
  databaseUrl: process.env.DATABASE_URL,
  jwtKey: process.env.JWT_KEY,
  // Base64 AES-256 key encrypting project secrets; unset disables secrets.
  secretEncryptionKey: process.env.SECRET_ENCRYPTION_KEY,
  webOrigin: process.env.WEB_ORIGIN,
  // Cluster-internal worker access for dashboard work: object dispatch
  // and typed reads over the WorkerData grpc service. Same secret the
  // workers use between themselves; development default matches theirs.
  worker: {
    grpcUrl: process.env.WORKER_GRPC_URL || 'localhost:3100',
    internalToken: process.env.INTERNAL_TOKEN || 'dev-internal-token',
  },
  inviteOnly: process.env.INVITE_ONLY === 'true',
});
