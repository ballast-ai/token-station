interface PageBackButtonProps {
  onClick: () => void;
  disabled?: boolean;
}

export default function PageBackButton({ onClick, disabled = false }: PageBackButtonProps) {
  return (
    <button className="page-back" type="button" disabled={disabled} onClick={onClick}>
      <span aria-hidden="true">←</span>
      <span>返回</span>
    </button>
  );
}
