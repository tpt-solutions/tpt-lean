import Mathlib.Data
import Std

namespace Demo
theorem t (a b : Nat) : a + b = b + a := by
  simp [Nat.add_comm]
  omega
def pair := (a,b)
-- comment kept
end Demo
