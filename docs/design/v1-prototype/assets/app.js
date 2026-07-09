// V1 原型共享脚本:侧边栏开合(移动端) + 明暗主题切换
// 逻辑照搬 cloud_ai_gateway src/templates/assets.rs 的 mobile_js / theme_toggle_script
function toggleSidebar() {
  document.querySelector('.sidebar').classList.toggle('open');
  document.querySelector('.sidebar-overlay').classList.toggle('open');
}
document.addEventListener('click', function (e) {
  if (e.target.classList.contains('sidebar-overlay')) toggleSidebar();
});

(function () {
  function applyTheme(t) {
    document.documentElement.setAttribute('data-theme', t);
    try { localStorage.setItem('theme', t); } catch (e) {}
  }
  document.addEventListener('click', function (e) {
    var btn = e.target.closest && e.target.closest('.theme-toggle');
    if (!btn) return;
    e.preventDefault();
    var current = document.documentElement.getAttribute('data-theme') || 'dark';
    applyTheme(current === 'light' ? 'dark' : 'light');
  });
})();
