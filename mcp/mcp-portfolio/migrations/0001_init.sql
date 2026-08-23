-- Esquema compartido por los cuatro MCP de la demo (portfolio, market-news,
-- kyc-compliance, crm-calendar). Cada servicio ejecuta esta migración al
-- arrancar, por lo que debe ser segura de repetir sin duplicar datos ni
-- fallar si otro servicio ya la aplicó primero.

CREATE TABLE IF NOT EXISTS bankers (
    id TEXT PRIMARY KEY,              -- coincide con preferred_username del JWT
    name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS clients (
    id TEXT PRIMARY KEY,
    banker_id TEXT NOT NULL REFERENCES bankers(id),
    name TEXT NOT NULL,
    risk_profile TEXT NOT NULL,       -- 'conservador' | 'moderado' | 'agresivo'
    kyc_status TEXT NOT NULL,         -- 'completo' | 'pendiente'
    pep_flag BOOLEAN NOT NULL DEFAULT FALSE,
    sector_focus TEXT
);

CREATE TABLE IF NOT EXISTS positions (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id),
    ticker TEXT NOT NULL,
    isin TEXT,
    sector TEXT NOT NULL,
    quantity NUMERIC NOT NULL,
    price NUMERIC NOT NULL,
    market_value NUMERIC NOT NULL,
    currency TEXT NOT NULL DEFAULT 'EUR'
);

CREATE TABLE IF NOT EXISTS transactions (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id),
    type TEXT NOT NULL,               -- 'buy' | 'sell' | 'deposit' | 'withdrawal'
    amount NUMERIC NOT NULL,
    origin_country TEXT,
    date TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS performance_snapshots (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id),
    period TEXT NOT NULL,             -- 'MTD' | 'QTD' | 'YTD'
    twr NUMERIC NOT NULL,
    benchmark_twr NUMERIC NOT NULL
);

CREATE TABLE IF NOT EXISTS products (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    risk_rating TEXT NOT NULL,
    asset_class TEXT NOT NULL,
    sector TEXT,
    min_investment NUMERIC NOT NULL
);

CREATE TABLE IF NOT EXISTS meetings (
    id TEXT PRIMARY KEY,
    banker_id TEXT NOT NULL REFERENCES bankers(id),
    client_id TEXT NOT NULL REFERENCES clients(id),
    datetime TIMESTAMPTZ NOT NULL,
    notes TEXT
);

-- Semillas idempotentes
INSERT INTO bankers (id, name) VALUES ('alice','Alice'), ('bob','Bob'), ('charlie','Charlie')
  ON CONFLICT (id) DO NOTHING;

INSERT INTO clients (id, banker_id, name, risk_profile, kyc_status, pep_flag, sector_focus) VALUES
  ('cli-001','bob','Clara Fontán','moderado','completo',false,'logística'),
  ('cli-002','bob','Grupo Delta Textil','agresivo','completo',false,'textil'),
  ('cli-003','bob','Marcus Wren','conservador','completo',false,'importación'),
  ('cli-004','alice','Elena Duarte','moderado','completo',false,'tecnología'),
  ('cli-005','charlie','Fundación Iris','conservador','pendiente',true,'salud')
  ON CONFLICT (id) DO NOTHING;

INSERT INTO positions (id, client_id, ticker, isin, sector, quantity, price, market_value, currency) VALUES
  ('pos-001','cli-001','NDFR','XX0000000001','logística',500,42.10,21050,'EUR'),
  ('pos-002','cli-001','OCLN','XX0000000002','logística',200,88.50,17700,'EUR'),
  ('pos-003','cli-002','DLTX','XX0000000003','textil',1000,15.30,15300,'EUR')
  ON CONFLICT (id) DO NOTHING;

INSERT INTO performance_snapshots (id, client_id, period, twr, benchmark_twr) VALUES
  ('perf-001','cli-001','MTD',0.021,0.015),
  ('perf-002','cli-002','MTD',-0.034,0.015)
  ON CONFLICT (id) DO NOTHING;
