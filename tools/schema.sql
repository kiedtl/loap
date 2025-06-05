CREATE TABLE Packages (
	id INTEGER PRIMARY KEY NOT NULL,
	name TEXT UNIQUE NOT NULL,
	maintainer INTEGER NOT NULL,
	repository INTEGER NOT NULL,
	
	FOREIGN KEY(maintainer)	REFERENCES Maintainers(id),
	FOREIGN KEY(repository) REFERENCES Repositories(id)
) STRICT;
CREATE TABLE IF NOT EXISTS "Repositories" (
	"id"	INTEGER NOT NULL,
	"forge_url"	TEXT NOT NULL,
	"org"	TEXT NOT NULL,
	"dir"	TEXT NOT NULL,
	"name"	TEXT NOT NULL,
	PRIMARY KEY("id")
) STRICT;
CREATE TABLE IF NOT EXISTS "Builds" (
	"id"	INTEGER NOT NULL,
	"built_by"	INTEGER NOT NULL,
	"package"	INTEGER NOT NULL,
	"version"	TEXT NOT NULL,
	"completed_at"	INTEGER NOT NULL,
	"completed_in"	INTEGER NOT NULL,
	"result"	INTEGER NOT NULL,
	"downloads"	INTEGER NOT NULL,
	"object_path"	TEXT NOT NULL,
	"size"	INTEGER NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY("built_by") REFERENCES "Maintainers"("id"),
	FOREIGN KEY("package") REFERENCES "Packages"("id")
) STRICT;
CREATE TABLE IF NOT EXISTS "Maintainers" (
	"id"	INTEGER NOT NULL,
	"name"	TEXT NOT NULL UNIQUE,
	"email"	TEXT NOT NULL UNIQUE,
	"pubkey"	TEXT UNIQUE,
	PRIMARY KEY("id")
) STRICT;
