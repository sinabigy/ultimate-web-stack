import { fireEvent, render, screen } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";
import { DataTable, LineChart } from "../../src/components/data";
import { Field } from "../../src/components/ui";

interface Row {
  id: string;
  name: string;
  n: number;
}
const rows: Row[] = Array.from({ length: 30 }, (_, i) => ({
  id: String(i),
  name: `item-${String(i).padStart(2, "0")}`,
  n: 30 - i,
}));
const cols = [
  { key: "name", header: "Name", cell: (r: Row) => r.name, sortValue: (r: Row) => r.name },
  { key: "n", header: "Count", cell: (r: Row) => String(r.n), sortValue: (r: Row) => r.n },
];

describe("DataTable", () => {
  it("paginates, sorts with aria-sort and filters", async () => {
    render(() => (
      <DataTable caption="Things" columns={cols} rows={rows} rowKey={(r) => r.id} pageSize={10} />
    ));
    expect(screen.getAllByRole("row")).toHaveLength(11); // header + 10
    expect(screen.getByText(/Page 1 of 3/)).toBeInTheDocument();
    const countHeader = screen.getByRole("button", { name: /Count/ });
    fireEvent.click(countHeader);
    expect(countHeader.closest("th")).toHaveAttribute("aria-sort", "ascending");
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("item-29");
    fireEvent.click(countHeader);
    expect(countHeader.closest("th")).toHaveAttribute("aria-sort", "descending");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText(/Page 2 of 3/)).toBeInTheDocument();
  });

  it("filters rows and shows an empty state", () => {
    render(() => (
      <DataTable caption="Things" columns={cols} rows={rows} rowKey={(r) => r.id} filterText="zzz" />
    ));
    expect(screen.getByText("No results")).toBeInTheDocument();
  });
});

describe("Field", () => {
  it("wires label, hint and error for assistive technology", () => {
    render(() => (
      <Field label="Email" hint="We never share it" error="Required">
        {(a) => <input id={a.id} aria-describedby={a.describedBy} aria-invalid={a.invalid} />}
      </Field>
    ));
    const input = screen.getByLabelText("Email");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription(/We never share it.*Required/);
    expect(screen.getByRole("alert")).toHaveTextContent("Required");
  });
});

describe("LineChart", () => {
  it("has an accessible summary and a data table alternative", () => {
    render(() => (
      <LineChart
        title="Calls"
        labels={["2026-10-01", "2026-10-02"]}
        series={[{ name: "OK", values: [3, 5] }]}
      />
    ));
    expect(screen.getByRole("img")).toHaveAccessibleName(/Calls\. OK: total 8/);
    expect(screen.getByRole("table")).toHaveTextContent("2026-10-02");
  });
});
