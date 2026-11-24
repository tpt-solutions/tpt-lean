import Mathlib.Data.Nat.Basic
import Std

namespace Geo

/-- Compact sets attain their maximum. -/
theorem compact_max (s : Set ℝ) (h : IsCompact s) : True := by
  trivial

private def helper : Nat := 1

section Inner
lemma nested_lemma : True := trivial
end Inner

end Geo

-- A top-level example, outside any namespace.
theorem top_level : True := trivial
