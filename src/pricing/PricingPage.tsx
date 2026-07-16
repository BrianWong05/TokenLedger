// Pricing tab — placeholder. The shell mounts this and passes nothing; the
// Pricing wave fills the container and pulls its own data through PricingPort
// (src/pricing/pricing.ts), so it never edits the shell.
export default function PricingPage() {
  return <div className="tl-page tl-page-pricing" />;
}
