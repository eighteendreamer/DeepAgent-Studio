import ReactDOM from "react-dom/client";
import "../i18n";
import "../styles.css";
import "./pet.css";
import { PetWindow } from "./PetWindow";

ReactDOM.createRoot(document.getElementById("root")!).render(<PetWindow />);
