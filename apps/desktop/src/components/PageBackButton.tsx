interface PageBackButtonProps {
  onClick: () => void;
}

export default function PageBackButton({ onClick }: PageBackButtonProps) {
  return (
    <button className="page-back" type="button" onClick={onClick}>
      <span aria-hidden="true">←</span>
      <span>返回</span>
    </button>
  );
}
