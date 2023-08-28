export default () => ({
  port: parseInt(process.env.PORT, 10),
  externalServices: {
    scriptServiceUri: process.env.SCRIPT_SERVICE_URL,
    kvServiceUri: process.env.KV_SERVICE_URL,
  },
  databaseUrl: process.env.DATABASE_URL,
  jwtKey: process.env.JWT_KEY,
});
