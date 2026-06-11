import type { Metadata } from "next";
import PageShell, { Crumbs } from "@/components/PageShell";
import DpiaContent from "@/components/DpiaContent";

export const metadata: Metadata = {
  title: "DPIA template — persona packs",
  description: "Data Protection Impact Assessment template for Furx persona packs: data flows, lawful basis, retention, sub-processors, rights, risks & mitigations.",
  alternates: { canonical: "https://furx.cloud/docs/dpia-persona-pack/" },
};

export default function DpiaPersonaPackPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "DPIA — persona packs" }]} />
      <DpiaContent focus="persona-pack" />
    </PageShell>
  );
}
