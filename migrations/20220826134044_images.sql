-- Add migration script here
CREATE TABLE IF NOT EXISTS images
(
    id VARCHAR(26) PRIMARY KEY NOT NULL,
    hash VARCHAR(20) NOT NULL,
    upvotes INTEGER DEFAULT 0,
    downvotes INTEGER DEFAULT 0
);
