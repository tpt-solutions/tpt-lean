class Shape (α : Type) where
  area : α → Nat

instance : Shape Nat := ⟨fun n => n * n⟩

theorem sumlemma (xs : List Nat) : (xs ++ []).length = xs.length := by
  simp

def bigsum : Nat := (List.range 200).foldl (· + ·) 0

theorem heavy (a b c d : Nat) (h : a < b) (h2 : c < d) : a + c < b + d := by
  omega

example : 2 + 2 = 4 := by decide
