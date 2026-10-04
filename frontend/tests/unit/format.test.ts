import { describe, expect, it } from "vitest";
import { describeAction, deviceLabel, fmtRelative, initials } from "../../src/lib/format";

describe("format helpers", () => {
  it("relative time", () => {
    const now = Date.parse("2026-10-04T12:00:00Z");
    expect(fmtRelative("2026-10-04T11:59:50Z", now)).toBe("just now");
    expect(fmtRelative("2026-10-04T10:00:00Z", now)).toMatch(/2 hours ago/);
    expect(fmtRelative(null, now)).toBe("never");
  });
  it("initials", () => {
    expect(initials("Ada Lovelace")).toBe("AL");
    expect(initials("ada")).toBe("A");
    expect(initials("  ")).toBe("?");
  });
  it("describes audit actions", () => {
    expect(describeAction("organization.member_added")).toBe("Member added");
  });
  it("device labels", () => {
    expect(
      deviceLabel("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit Chrome/130 Safari/537"),
    ).toBe("Chrome on macOS");
    expect(deviceLabel(null)).toBe("Unknown device");
  });
});
