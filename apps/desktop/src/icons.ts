// Central Font Awesome registration so the bundle only ships the icons we use.
import { library } from "@fortawesome/fontawesome-svg-core";
import {
  faChevronDown, faMagnifyingGlassChart, faEllipsis, faThumbtack, faPen, faBoxArchive,
  faChevronRight, faChevronUp, faCodeBranch, faArrowUpRightFromSquare, faCircleInfo, faGlobe, faXmark,
  faPlus, faTerminal, faCheck, faArrowUp, faArrowDown, faArrowLeft, faArrowRight, faRotateRight,
  faExpand, faMagnifyingGlass, faCaretUp, faCaretDown, faCode, faGear, faLayerGroup,
  faPuzzlePiece, faCircleUser, faArrowRightFromBracket, faCube, faFileCsv, faFileWord, faArrowPointer,
  faFileLines as faFileLinesSolid, faEnvelope, faFileExcel, faRobot, faCloud, faBullseye,
  faBorderAll, faMinus, faSliders, faServer, faAnchor, faLink, faLeaf, faDesktop,
  faCircleCheck as faCircleCheckSolid, faLaptop, faCompress, faFolderPlus, faFileExport, faClockRotateLeft,
  faKey, faLock, faMoon, faCircleNotch, faHand, faCircleExclamation, faWrench, faListCheck, faShieldHalved,
  faBook, faToggleOn, faToggleOff, faTriangleExclamation, faNoteSticky, faTrash, faShareNodes, faList, faInbox, faLightbulb, faStop, faBolt, faCoins, faWallet,
  faStar, faPlay, faFolderTree, faMicrophone, faPause, faDownload, faTable, faAnglesLeft, faAnglesRight
} from "@fortawesome/free-solid-svg-icons";

import {
  faClock, faBell, faCalendarCheck, faWindowRestore, faCopy, faFileLines as faFileLinesRegular,
  faFolderOpen, faCommentDots, faComments, faPenToSquare, faFolder, faMessage, faSquare,
  faCircleCheck as faCircleCheckRegular, faSun, faFaceSmile, faKeyboard, faCompass, faTrashCan, faWindowMaximize,
  faImage, faFilePdf, faFileWord as faFileWordRegular, faFileExcel as faFileExcelRegular, faFilePowerpoint as faFilePowerpointRegular
} from "@fortawesome/free-regular-svg-icons";

import { faChrome, faFigma, faGitAlt, faGithub } from "@fortawesome/free-brands-svg-icons";

const solidIcons = [
  faChevronDown, faMagnifyingGlassChart, faEllipsis, faThumbtack, faPen, faBoxArchive,
  faChevronRight, faChevronUp, faCodeBranch, faArrowUpRightFromSquare, faCircleInfo, faGlobe, faXmark,
  faPlus, faTerminal, faCheck, faArrowUp, faArrowDown, faArrowLeft, faArrowRight, faRotateRight,
  faExpand, faMagnifyingGlass, faCaretUp, faCaretDown, faCode, faGear, faLayerGroup,
  faPuzzlePiece, faCircleUser, faArrowRightFromBracket, faCube, faFileCsv, faFileWord, faArrowPointer,
  faFileLinesSolid, faEnvelope, faFileExcel, faRobot, faCloud, faBullseye, faBorderAll, faMinus,
  faSliders, faServer, faAnchor, faLink, faLeaf, faDesktop, faCircleCheckSolid, faLaptop,
  faCompress, faFolderPlus, faFileExport, faClockRotateLeft, faKey, faLock, faMoon, faCircleNotch, faHand, faCircleExclamation, faWrench, faListCheck, faShieldHalved,
  faBook, faToggleOn, faToggleOff, faTriangleExclamation, faNoteSticky, faTrash, faShareNodes, faList, faInbox, faLightbulb, faStop, faBolt, faCoins, faWallet,
  faStar, faPlay, faFolderTree, faMicrophone, faPause, faDownload, faTable, faAnglesLeft, faAnglesRight
];

const regularIcons = [
  faClock, faBell, faCalendarCheck, faWindowRestore, faCopy, faFileLinesRegular,
  faFolderOpen, faCommentDots, faComments, faPenToSquare, faFolder, faMessage, faSquare,
  faCircleCheckRegular, faSun, faFaceSmile, faKeyboard, faCompass, faTrashCan, faWindowMaximize,
  faImage, faFilePdf, faFileWordRegular, faFileExcelRegular, faFilePowerpointRegular
];

const brandIcons = [
  faChrome, faFigma, faGitAlt, faGithub
];

solidIcons.forEach(icon => library.add(icon));
regularIcons.forEach(icon => library.add(icon));
brandIcons.forEach(icon => library.add(icon));
