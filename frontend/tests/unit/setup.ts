import "@testing-library/jest-dom/vitest";
import { cleanup } from "@solidjs/testing-library";
import { afterEach } from "vitest";

// Vitest globals are off, so register Testing Library cleanup explicitly.
afterEach(() => cleanup());
