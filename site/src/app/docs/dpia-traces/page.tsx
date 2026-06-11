import type { Metadata } from "next";
import PageShell, { Crumbs } from "@/components/PageShell";
import DpiaContent from "@/components/DpiaContent";

export const metadata: Metadata = {
  title: "DPIA template — cloud traces",
  description: "Data Protection Impact Assessment template for Furx cloud traces: data flows, lawful basis, retention, sub-processors, rights, risks & mitigations.",
  alternates: { canonical: "https://furx.cloud/docs/dpia-traces/" },
};

export default function DpiaTracesPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "DPIA — traces" }]} />
      <DpiaContent focus="traces" />
    </PageShell>
  );
}
