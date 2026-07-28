import { useLocalizedCopy } from "./LanguageProvider";

interface PageBackButtonProps {
  onClick: () => void;
  disabled?: boolean;
}

export default function PageBackButton({ onClick, disabled = false }: PageBackButtonProps) {
  const { copy } = useLocalizedCopy();
  return (
    <button className="page-back" type="button" disabled={disabled} onClick={onClick}>
      <span aria-hidden="true">←</span>
      <span>{copy("Back", "返回")}</span>
    </button>
  );
}
