import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { checkFileAccess } from "./file-guard";

describe("File Access Guard", () => {
  it("blocks .env files", () => {
    const result = checkFileAccess("/project/.env");
    assert.equal(result.allowed, false);
  });

  it("blocks .env.production", () => {
    const result = checkFileAccess("/project/.env.production");
    assert.equal(result.allowed, false);
  });

  it("blocks SSH keys", () => {
    const result = checkFileAccess("/home/user/.ssh/id_rsa");
    assert.equal(result.allowed, false);
  });

  it("blocks AWS credentials", () => {
    const result = checkFileAccess("/home/user/.aws/credentials");
    assert.equal(result.allowed, false);
  });

  it("blocks credentials.json", () => {
    const result = checkFileAccess("/project/credentials.json");
    assert.equal(result.allowed, false);
  });

  it("blocks sqlite databases", () => {
    const result = checkFileAccess("/data/users.sqlite3");
    assert.equal(result.allowed, false);
  });

  it("blocks OpenObscure encrypted files", () => {
    const result = checkFileAccess("/data/openobscure-session.enc.json");
    assert.equal(result.allowed, false);
  });

  it("allows regular source files", () => {
    assert.equal(checkFileAccess("/project/src/main.ts").allowed, true);
    assert.equal(checkFileAccess("/project/README.md").allowed, true);
    assert.equal(checkFileAccess("/project/package.json").allowed, true);
  });

  it("respects custom deny patterns", () => {
    const config = { extraDenyPatterns: ["\\.secret$"] };
    const result = checkFileAccess("/project/api.secret", config);
    assert.equal(result.allowed, false);
  });

  it("respects explicit allow overrides", () => {
    const config = { allowPatterns: ["test\\.env$"] };
    const result = checkFileAccess("/project/test.env", config);
    assert.equal(result.allowed, true);
  });

  it("normalizes Windows paths", () => {
    const result = checkFileAccess("C:\\Users\\admin\\.ssh\\id_rsa");
    assert.equal(result.allowed, false);
  });
});
