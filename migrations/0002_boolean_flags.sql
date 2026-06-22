ALTER TABLE client_keys
    ALTER COLUMN enabled DROP DEFAULT,
    ALTER COLUMN enabled TYPE BOOLEAN USING enabled <> 0,
    ALTER COLUMN enabled SET DEFAULT TRUE,
    ALTER COLUMN enabled SET NOT NULL,
    ALTER COLUMN allow_extra_usage DROP DEFAULT,
    ALTER COLUMN allow_extra_usage TYPE BOOLEAN USING allow_extra_usage <> 0,
    ALTER COLUMN allow_extra_usage SET DEFAULT FALSE,
    ALTER COLUMN allow_extra_usage SET NOT NULL;

ALTER TABLE models
    ALTER COLUMN enabled DROP DEFAULT,
    ALTER COLUMN enabled TYPE BOOLEAN USING enabled <> 0,
    ALTER COLUMN enabled SET DEFAULT TRUE,
    ALTER COLUMN enabled SET NOT NULL;
