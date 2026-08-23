-- Semillas de meetings propias de mcp-crm-calendar. Se mantienen en una
-- migración separada (versión 2) en vez de añadirlas a 0001_init.sql para
-- que ese fichero siga siendo byte-idéntico en los cuatro MCP: sqlx guarda
-- un checksum por versión en la tabla compartida _sqlx_migrations, y dos
-- servicios aplicando contenido distinto bajo la misma versión producirían
-- un VersionMismatch en el que arranque después.

INSERT INTO meetings (id, banker_id, client_id, datetime, notes) VALUES
  ('mtg-001','bob','cli-001','2026-08-24T10:00:00Z','Revisar exposición a logística tras subida de tarifas portuarias comentada la última vez.'),
  ('mtg-002','bob','cli-002','2026-08-25T16:30:00Z',NULL),
  ('mtg-003','alice','cli-004','2026-08-24T09:00:00Z',NULL)
  ON CONFLICT (id) DO NOTHING;
