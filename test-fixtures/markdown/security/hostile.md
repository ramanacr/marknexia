# Hostile Markdown Test

<script>alert("pwned");</script>
<img src="x" onerror="alert('xss')" />
<a href="javascript:alert(1)">Click Me</a>
<a href="vbscript:msgbox(1)">Click VB</a>
<iframe src="https://evil.com"></iframe>

<svg width="100" height="100">
  <circle cx="50" cy="50" r="40" stroke="green" stroke-width="4" fill="yellow" />
  <script>alert("svg xss");</script>
</svg>
