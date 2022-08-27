-- Add migration script here
CREATE TABLE IF NOT EXISTS images
(
    id         VARCHAR(26)  PRIMARY KEY NOT NULL,
    filename   VARCHAR(200) NOT NULL,
    hash       VARCHAR(20)  NOT NULL,
    confidence REAL         DEFAULT 0,
    sorting    REAL         DEFAULT 0,
    upvotes    INTEGER      DEFAULT 0,
    downvotes  INTEGER      DEFAULT 0
);

CREATE INDEX IF NOT EXISTS sorting ON images ( sorting );
