(ns bank-account
  "Simple bank account with deposit, withdraw, and balance.")

(defn create-account
  ([holder] (create-account holder 0.0))
  ([holder initial-balance]
   {:holder holder :balance initial-balance :transactions []}))

(defn deposit [account amount]
  (-> account
      (update :balance + amount)
      (update :transactions conj {:type :deposit :amount amount})))

(defn withdraw [account amount]
  (if (<= amount (:balance account))
    (-> account
        (update :balance - amount)
        (update :transactions conj {:type :withdraw :amount amount}))
    (throw (Exception. "Insufficient funds"))))
